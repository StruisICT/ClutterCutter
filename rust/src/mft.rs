// NTFS Master File Table scanner — direct port of MftScanner in ClutterCutter.cs.
// Opens \\.\<drive>: (requires admin), reads the MFT in 4 MB chunks via raw volume
// reads, parses each FILE record's attributes, then assembles a FolderNode tree
// from parent-FRN links. ~5–10x faster than the FindFirstFileEx walker because it
// reads metadata in one big sequential pass instead of per-folder syscalls.

use crate::scanner::{wide, ProgressFn};
use crate::types::{FileEntry, FolderNode, ScanProgress};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetVolumeInformationW, ReadFile, SetFilePointerEx, FILE_BEGIN,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{FSCTL_GET_NTFS_VOLUME_DATA, NTFS_VOLUME_DATA_BUFFER};
use windows::Win32::System::IO::DeviceIoControl;

const GENERIC_READ: u32 = 0x80000000;

// ----------------------------------------------------------------------------
// Public API
// ----------------------------------------------------------------------------

pub struct MftScanner {
    cancel: Arc<AtomicBool>,
    progress: Option<Arc<ProgressFn>>,
    track_files: bool,

    files_scanned: AtomicI64,
    total_size: AtomicI64,
    last_report_ms: AtomicI64,
    mft_bytes_total: AtomicI64,
    mft_bytes_read: AtomicI64,
    start: Instant,
}

impl Default for MftScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl MftScanner {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            track_files: false,
            files_scanned: AtomicI64::new(0),
            total_size: AtomicI64::new(0),
            last_report_ms: AtomicI64::new(0),
            mft_bytes_total: AtomicI64::new(0),
            mft_bytes_read: AtomicI64::new(0),
            start: Instant::now(),
        }
    }

    pub fn with_cancel(mut self, c: Arc<AtomicBool>) -> Self {
        self.cancel = c;
        self
    }

    pub fn with_progress(mut self, p: ProgressFn) -> Self {
        self.progress = Some(Arc::new(p));
        self
    }

    pub fn with_track_files(mut self, b: bool) -> Self {
        self.track_files = b;
        self
    }

    pub fn scan(&self, root: &str) -> Result<FolderNode, String> {
        let norm = root.trim_end_matches('\\').trim();
        if norm.len() < 2 || norm.as_bytes()[1] != b':' {
            return Err("MFT scan requires a drive-letter root (e.g. C:)".into());
        }
        let drive = (norm.as_bytes()[0] as char).to_ascii_uppercase();
        let vol_path = format!("\\\\.\\{drive}:");

        let vol_w = wide(&vol_path);
        let h = unsafe {
            CreateFileW(
                PCWSTR(vol_w.as_ptr()),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                // FILE_FLAG_SEQUENTIAL_SCAN — hint the cache manager for the
                // one-pass streaming read of the MFT.
                FILE_FLAGS_AND_ATTRIBUTES(0x0800_0000),
                None,
            )
        }
        .map_err(|e| format!("Cannot open {vol_path} (run as Administrator): {e}"))?;

        if h.is_invalid() || h == INVALID_HANDLE_VALUE {
            return Err(format!("Cannot open {vol_path} (run as Administrator)"));
        }
        let _guard = HandleGuard(h);

        // 1. NTFS volume parameters
        let vd = unsafe { get_volume_data(h) }?;
        let record_size = vd.BytesPerFileRecordSegment as usize;
        let sector_size = vd.BytesPerSector as usize;
        let bytes_per_cluster = vd.BytesPerCluster as i64;

        // Validate the geometry before it feeds divisions and allocations. A
        // corrupt/hostile volume reporting a zero or absurd record/sector size
        // would otherwise divide-by-zero (a panic == crash under panic=abort) or
        // try to allocate a wild buffer. Bail to the walker fallback instead.
        if record_size == 0
            || sector_size == 0
            || !record_size.is_multiple_of(sector_size)
            || record_size > 64 * 1024
        {
            return Err(format!(
                "Unexpected NTFS geometry (record={record_size}, sector={sector_size})"
            ));
        }

        // 2. Read MFT record 0 → parse its $DATA data runs (where the MFT itself lives)
        let mut rec0 = vec![0u8; record_size];
        unsafe { read_at(h, vd.MftStartLcn * bytes_per_cluster, &mut rec0)? };
        apply_fixups(&mut rec0, 0, record_size, sector_size);
        let runs = extract_mft_data_runs(&rec0);
        if runs.is_empty() {
            return Err("Could not parse MFT data runs from record 0.".into());
        }

        // 3. Bulk-read the MFT, parsing records in parallel into a Vec indexed
        //    by FRN. The FRN is exactly the record's ordinal position, so a Vec
        //    (no hashing) suffices and parsing splits cleanly across cores.
        let n_records = (vd.MftValidDataLength as i64 / record_size as i64).max(0) as usize;
        // Guard against a corrupt/hostile volume reporting an absurd MFT size,
        // which would otherwise force a multi-hundred-GB allocation (OOM abort).
        // 256M records dwarfs any real volume; bail to the walker fallback instead.
        const MAX_MFT_RECORDS: usize = 256 * 1024 * 1024;
        if n_records > MAX_MFT_RECORDS {
            return Err(format!(
                "MFT reports {n_records} records — implausibly large, using walker instead"
            ));
        }
        let mut entries: Vec<Option<MftEntry>> = Vec::new();
        entries.resize_with(n_records, || None);
        let mut frn_cursor: usize = 0;
        let mut total_bytes_remaining = vd.MftValidDataLength as i64;
        self.mft_bytes_total
            .store(vd.MftValidDataLength as i64, Ordering::SeqCst);
        self.mft_bytes_read.store(0, Ordering::SeqCst);
        // Bigger reads = fewer syscalls on the streaming pass.
        const CHUNK: usize = 16 * 1024 * 1024;
        let mut buf = vec![0u8; CHUNK];
        let nthreads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(1);

        for run in &runs {
            if self.cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if total_bytes_remaining <= 0 {
                break;
            }
            let mut pos = run.lcn * bytes_per_cluster;
            let mut run_bytes = run.length * bytes_per_cluster;
            while run_bytes > 0 && total_bytes_remaining > 0 {
                if self.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let want = (CHUNK as i64).min(run_bytes).min(total_bytes_remaining);
                let to_read = (want - (want % record_size as i64)) as usize;
                if to_read == 0 {
                    break;
                }

                unsafe { set_pos(h, pos)? };
                let mut bytes_read: u32 = 0;
                unsafe { ReadFile(h, Some(&mut buf[..to_read]), Some(&mut bytes_read), None) }
                    .map_err(|e| format!("ReadFile MFT failed: {e}"))?;
                if bytes_read == 0 {
                    break;
                }
                let mut n_recs = (bytes_read as usize) / record_size;
                // Never write past the FRN space we sized `entries` for.
                n_recs = n_recs.min(entries.len() - frn_cursor);
                if n_recs > 0 {
                    let process_bytes = n_recs * record_size;
                    let (files, size) = parse_chunk_parallel(
                        &mut buf[..process_bytes],
                        record_size,
                        sector_size,
                        &mut entries[frn_cursor..frn_cursor + n_recs],
                        nthreads,
                    );
                    self.files_scanned.fetch_add(files, Ordering::Relaxed);
                    self.total_size.fetch_add(size, Ordering::Relaxed);
                    frn_cursor += n_recs;
                }
                pos += bytes_read as i64;
                run_bytes -= bytes_read as i64;
                total_bytes_remaining -= bytes_read as i64;
                self.mft_bytes_read
                    .fetch_add(bytes_read as i64, Ordering::Relaxed);
                self.report_progress();
            }
        }

        // 4. Wire FRN children, build FolderNode tree from root FRN=5
        self.build_tree(entries, drive)
    }

    fn build_tree(
        &self,
        entries: Vec<Option<MftEntry>>,
        drive: char,
    ) -> Result<FolderNode, String> {
        if entries.get(5).and_then(|e| e.as_ref()).is_none() {
            return Err("MFT root entry (FRN 5) not found.".into());
        }

        // child FRNs per parent (FRN == Vec index).
        let mut kids: HashMap<i64, Vec<i64>> = HashMap::with_capacity(entries.len() / 4);
        for (frn, slot) in entries.iter().enumerate() {
            if frn == 5 {
                continue;
            }
            let e = match slot {
                Some(e) => e,
                None => continue,
            };
            let pf = e.parent_frn;
            if pf >= 0 && (pf as usize) < entries.len() && entries[pf as usize].is_some() {
                kids.entry(pf).or_default().push(frn as i64);
            }
        }

        let root_path = format!("{drive}:\\");
        let mut root_node = FolderNode {
            full_path: root_path.clone(),
            name: root_path.clone(),
            last_modified_ft: entries[5].as_ref().unwrap().last_write_ft,
            ..Default::default()
        };

        build_subtree(
            5,
            &mut root_node,
            &root_path,
            &entries,
            &kids,
            self.track_files,
        );
        Ok(root_node)
    }

    fn report_progress(&self) {
        let progress = match &self.progress {
            Some(p) => p,
            None => return,
        };
        let now_ms = self.start.elapsed().as_millis() as i64;
        let last = self.last_report_ms.load(Ordering::Relaxed);
        if now_ms - last < 80 {
            return;
        }
        if self
            .last_report_ms
            .compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let total = self.total_size.load(Ordering::Relaxed);
        let files = self.files_scanned.load(Ordering::Relaxed);
        let mft_total = self.mft_bytes_total.load(Ordering::Relaxed);
        // Reserve the last 5% for tree-build; cap read-progress at 95%.
        let percent = if mft_total > 0 {
            let pct =
                95.0 * (self.mft_bytes_read.load(Ordering::Relaxed) as f64) / (mft_total as f64);
            pct.clamp(0.0, 95.0)
        } else {
            -1.0
        };
        progress(&ScanProgress {
            total_size: total,
            files_scanned: files,
            current_path: "MFT scan in progress...".to_string(),
            percent,
        });
    }
}

// Applies USA fixups and parses the records of one MFT read-chunk in parallel,
// writing each into its FRN slot. Records map 1:1 and positionally to `out`, so
// the slices split cleanly across threads with no shared state. Returns the
// (file count, total file size) contributed by this chunk.
fn parse_chunk_parallel(
    buf: &mut [u8],
    rec_size: usize,
    sector_size: usize,
    out: &mut [Option<MftEntry>],
    nthreads: usize,
) -> (i64, i64) {
    let n = out.len();
    if n == 0 {
        return (0, 0);
    }
    let nthreads = nthreads.min(n).max(1);
    let per = n.div_ceil(nthreads);

    // Each (buf, out) chunk pair covers the same `per` records, so both split
    // at identical record boundaries.
    std::thread::scope(|s| {
        let handles: Vec<_> = buf
            .chunks_mut(per * rec_size)
            .zip(out.chunks_mut(per))
            .map(|(bc, oc)| {
                s.spawn(move || {
                    let (mut files, mut size) = (0i64, 0i64);
                    for (i, slot) in oc.iter_mut().enumerate() {
                        let off = i * rec_size;
                        apply_fixups(bc, off, rec_size, sector_size);
                        if let Some(e) = parse_record(bc, off, rec_size) {
                            if !e.is_dir {
                                files += 1;
                                size += e.size;
                            }
                            *slot = Some(e);
                        }
                    }
                    (files, size)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .fold((0i64, 0i64), |(a, b), (c, d)| (a + c, b + d))
    })
}

// Parses one in-use FILE record into an MftEntry (or None for free/invalid
// records or ones with no name). Pure — no shared state, so it runs on any
// thread. `off` is relative to `buf`.
fn parse_record(buf: &[u8], off: usize, rec_size: usize) -> Option<MftEntry> {
    // The fixed header fields we read live within the first 42 bytes; a record
    // smaller than that (or one that runs past the buffer) is malformed.
    if rec_size < 42 || off + rec_size > buf.len() {
        return None;
    }
    if &buf[off..off + 4] != b"FILE" {
        return None;
    }

    let flags = u16_le(buf, off + 22);
    let in_use = (flags & 0x01) != 0;
    let is_dir = (flags & 0x02) != 0;
    if !in_use {
        return None;
    }

    let first_attr_off = u16_le(buf, off + 20) as usize;
    let mut p = off + first_attr_off;
    let rec_end = off + rec_size;

    let mut best_name: Option<String> = None;
    let mut best_ns: u8 = 0xFF;
    let mut parent_frn: i64 = -1;
    let mut size: i64 = 0;
    let mut size_found = false;
    let mut last_write_ft: i64 = 0;
    let mut is_reparse = false;

    // Require a full 24-byte resident attribute header before reading any of its
    // fields (value-length at p+16, value-offset at p+20). The bytes come straight
    // off disk, so a truncated/crafted attribute must not read past the record.
    while p + 24 <= rec_end {
        let attr_type = u32_le(buf, p);
        if attr_type == 0xFFFFFFFF {
            break;
        }
        let alen = u32_le(buf, p + 4) as usize;
        if alen == 0 || p + alen > rec_end {
            break;
        }
        let non_resident = buf[p + 8];
        let attr_name_len = buf[p + 9];

        if attr_type == 0x10 && non_resident == 0 {
            // $STANDARD_INFORMATION (resident) — modification time at value+8,
            // the DOS file-attributes DWORD at value+32 (carries the
            // reparse-point bit for junctions/symlinks).
            let v_off = u16_le(buf, p + 20) as usize;
            let v = p + v_off;
            if v + 16 <= rec_end && last_write_ft == 0 {
                last_write_ft = i64_le(buf, v + 8);
            }
            if v + 36 <= rec_end {
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
                is_reparse = (u32_le(buf, v + 32) & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
            }
        } else if attr_type == 0x30 && non_resident == 0 {
            // $FILE_NAME (resident) — prefer Win32 / Win32&DOS namespace
            let v_off = u16_le(buf, p + 20) as usize;
            let v = p + v_off;
            if v + 66 <= rec_end {
                let parent_raw = i64_le(buf, v);
                let pfrn = parent_raw & 0x0000_FFFF_FFFF_FFFF;
                let mod_ft = i64_le(buf, v + 16);
                let name_len = buf[v + 64] as usize;
                let ns = buf[v + 65];
                let name_byte_len = name_len * 2;
                if v + 66 + name_byte_len <= rec_end {
                    let name = utf16le_to_string(&buf[v + 66..v + 66 + name_byte_len]);
                    let prio = name_priority(ns);
                    let cur_prio = if best_name.is_some() {
                        name_priority(best_ns)
                    } else {
                        -1
                    };
                    if prio > cur_prio {
                        best_name = Some(name);
                        best_ns = ns;
                        parent_frn = pfrn;
                        if last_write_ft == 0 {
                            last_write_ft = mod_ft;
                        }
                    }
                }
            }
        } else if attr_type == 0x80 && attr_name_len == 0 && !size_found {
            // $DATA, default unnamed stream — first occurrence wins
            if non_resident == 0 {
                size = u32_le(buf, p + 16) as i64;
            } else if p + 56 <= rec_end {
                size = i64_le(buf, p + 48);
            }
            size_found = true;
        }

        p += alen;
    }

    let name = best_name?;
    if is_dir {
        size = 0;
    }
    Some(MftEntry {
        parent_frn,
        name,
        size,
        is_dir,
        is_reparse,
        last_write_ft,
    })
}

// ----------------------------------------------------------------------------
// is_ntfs_drive_root — caller checks this before invoking MftScanner.
// ----------------------------------------------------------------------------

#[allow(dead_code)] // called by the GUI layer; main.rs uses --mft for now
pub fn is_ntfs_drive_root(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let mut trimmed = path.trim().to_string();
    if trimmed.len() == 2 && trimmed.as_bytes()[1] == b':' {
        trimmed.push('\\');
    }
    if trimmed.len() != 3 || trimmed.as_bytes()[1] != b':' || trimmed.as_bytes()[2] != b'\\' {
        return false;
    }
    let root_w = wide(&trimmed);
    let mut fs_buf = [0u16; 20];
    let mut label_buf = [0u16; 64];
    let mut serial: u32 = 0;
    let mut max_len: u32 = 0;
    let mut flags: u32 = 0;
    let ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(root_w.as_ptr()),
            Some(&mut label_buf),
            Some(&mut serial),
            Some(&mut max_len),
            Some(&mut flags),
            Some(&mut fs_buf),
        )
    };
    if ok.is_err() {
        return false;
    }
    let fs = crate::scanner::wstr_to_string(&fs_buf);
    fs.eq_ignore_ascii_case("NTFS")
}

// ----------------------------------------------------------------------------
// Internal data structures
// ----------------------------------------------------------------------------

struct MftEntry {
    parent_frn: i64,
    name: String,
    size: i64,
    is_dir: bool,
    is_reparse: bool,
    last_write_ft: i64,
}

#[derive(Copy, Clone)]
struct DataRun {
    lcn: i64,
    length: i64,
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// ----------------------------------------------------------------------------
// Volume / I/O helpers
// ----------------------------------------------------------------------------

unsafe fn get_volume_data(h: HANDLE) -> Result<NTFS_VOLUME_DATA_BUFFER, String> {
    let mut out: NTFS_VOLUME_DATA_BUFFER = std::mem::zeroed();
    let sz = std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32;
    let mut returned: u32 = 0;
    DeviceIoControl(
        h,
        FSCTL_GET_NTFS_VOLUME_DATA,
        None,
        0,
        Some(&mut out as *mut _ as *mut _),
        sz,
        Some(&mut returned),
        None,
    )
    .map_err(|e| format!("FSCTL_GET_NTFS_VOLUME_DATA failed: {e}"))?;
    Ok(out)
}

unsafe fn set_pos(h: HANDLE, pos: i64) -> Result<(), String> {
    let mut np: i64 = 0;
    SetFilePointerEx(h, pos, Some(&mut np), FILE_BEGIN)
        .map_err(|e| format!("SetFilePointerEx({pos}) failed: {e}"))
}

unsafe fn read_at(h: HANDLE, pos: i64, buf: &mut [u8]) -> Result<(), String> {
    set_pos(h, pos)?;
    let mut got: u32 = 0;
    ReadFile(h, Some(buf), Some(&mut got), None)
        .map_err(|e| format!("ReadFile at {pos} failed: {e}"))
}

// ----------------------------------------------------------------------------
// USA fixups (NTFS multi-sector transfer protection)
// ----------------------------------------------------------------------------

fn apply_fixups(buf: &mut [u8], rec_off: usize, rec_size: usize, sector_size: usize) {
    if buf.len() < rec_off + 8 {
        return;
    }
    if &buf[rec_off..rec_off + 4] != b"FILE" {
        return;
    }
    let usa_offset = u16_le(buf, rec_off + 4) as usize;
    let usa_count = u16_le(buf, rec_off + 6) as usize;
    if usa_count < 1 {
        return;
    }
    let usa_pos = rec_off + usa_offset;
    for i in 1..usa_count {
        let sector_end = rec_off + i * sector_size - 2;
        if sector_end + 2 > rec_off + rec_size {
            break;
        }
        let src = usa_pos + i * 2;
        if src + 2 > buf.len() {
            break;
        }
        buf[sector_end] = buf[src];
        buf[sector_end + 1] = buf[src + 1];
    }
}

// ----------------------------------------------------------------------------
// Find the MFT's own $DATA data runs (record 0)
// ----------------------------------------------------------------------------

fn extract_mft_data_runs(rec: &[u8]) -> Vec<DataRun> {
    if rec.len() < 42 {
        return Vec::new();
    }
    let first_attr = u16_le(rec, 20) as usize;
    let mut p = first_attr;
    // p+10 keeps the type/len/flags header reads (up to p+9) in bounds.
    while p + 10 <= rec.len() {
        let attr_type = u32_le(rec, p);
        if attr_type == 0xFFFFFFFF {
            break;
        }
        let alen = u32_le(rec, p + 4) as usize;
        if alen == 0 || p + alen > rec.len() {
            break;
        }
        // Non-resident (buf[p+8]==1) unnamed (buf[p+9]==0) $DATA (type 0x80). The
        // run-list offset lives at p+32, so require the non-resident header too.
        if attr_type == 0x80 && rec[p + 8] == 1 && rec[p + 9] == 0 && p + 34 <= rec.len() {
            let run_offset = u16_le(rec, p + 32) as usize;
            return parse_data_runs(rec, p + run_offset, p + alen);
        }
        p += alen;
    }
    Vec::new()
}

fn parse_data_runs(buf: &[u8], start: usize, end: usize) -> Vec<DataRun> {
    let mut runs = Vec::new();
    let mut prev_lcn: i64 = 0;
    let mut p = start;
    while p < end {
        let header = buf[p];
        p += 1;
        if header == 0 {
            break;
        }
        let len_bytes = (header & 0x0F) as usize;
        let off_bytes = ((header >> 4) & 0x0F) as usize;
        // NTFS never encodes a run field wider than 8 bytes; a larger nibble is
        // corrupt and would make read_signed_le shift by >= 64 (UB-ish / garbage).
        if len_bytes == 0 || len_bytes > 8 {
            break;
        }
        if p + len_bytes > end {
            break;
        }
        let length = read_signed_le(buf, p, len_bytes);
        p += len_bytes;
        if off_bytes == 0 {
            // sparse run — skip
            continue;
        }
        if off_bytes > 8 || p + off_bytes > end {
            break;
        }
        let offset = read_signed_le(buf, p, off_bytes);
        p += off_bytes;
        let lcn = prev_lcn + offset;
        prev_lcn = lcn;
        runs.push(DataRun { lcn, length });
    }
    runs
}

fn read_signed_le(buf: &[u8], off: usize, len: usize) -> i64 {
    let mut v: i64 = 0;
    for i in 0..len {
        v |= (buf[off + i] as i64) << (i * 8);
    }
    if len < 8 && (buf[off + len - 1] & 0x80) != 0 {
        v |= !((1i64 << (len * 8)) - 1);
    }
    v
}

// ----------------------------------------------------------------------------
// Tree assembly
// ----------------------------------------------------------------------------

fn build_subtree(
    frn: i64,
    node: &mut FolderNode,
    node_path: &str,
    entries: &[Option<MftEntry>],
    kids: &HashMap<i64, Vec<i64>>,
    track_files: bool,
) {
    let kid_frns = match kids.get(&frn) {
        Some(v) => v,
        None => return,
    };
    for &child_frn in kid_frns {
        let c = match entries.get(child_frn as usize).and_then(|e| e.as_ref()) {
            Some(e) => e,
            None => continue,
        };
        // Skip directory junctions / symlinks: the target lives elsewhere in the
        // tree, so recursing would double-count it (and attach a subtree under
        // the wrong path). The FindFirstFile walker skips reparse dirs the same
        // way, so both scan paths agree.
        if c.is_dir && c.is_reparse {
            continue;
        }
        if c.is_dir {
            let child_path = if node_path.ends_with('\\') {
                format!("{node_path}{}", c.name)
            } else {
                format!("{node_path}\\{}", c.name)
            };
            let mut child = FolderNode {
                full_path: child_path.clone(),
                name: c.name.clone(),
                last_modified_ft: c.last_write_ft,
                ..Default::default()
            };
            build_subtree(
                child_frn,
                &mut child,
                &child_path,
                entries,
                kids,
                track_files,
            );
            node.size += child.size;
            node.file_count += child.file_count;
            node.folder_count += child.folder_count + 1;
            node.children.push(child);
        } else {
            node.own_size += c.size;
            node.size += c.size;
            node.direct_file_count += 1;
            node.file_count += 1;
            if track_files {
                node.files.push(FileEntry {
                    name: c.name.clone(),
                    size: c.size,
                    last_modified_ft: c.last_write_ft,
                });
            }
        }
    }
}

fn name_priority(ns: u8) -> i32 {
    // Win32&DOS combined > Win32 > POSIX > DOS
    match ns {
        3 => 4,
        1 => 3,
        0 => 2,
        2 => 1,
        _ => 0,
    }
}

// ----------------------------------------------------------------------------
// Byte helpers
// ----------------------------------------------------------------------------

#[inline]
fn u16_le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

#[inline]
fn u32_le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[inline]
fn i64_le(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes([
        b[o],
        b[o + 1],
        b[o + 2],
        b[o + 3],
        b[o + 4],
        b[o + 5],
        b[o + 6],
        b[o + 7],
    ])
}

fn utf16le_to_string(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn little_endian_readers() {
        let b = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(u16_le(&b, 0), 0x0201);
        assert_eq!(u32_le(&b, 0), 0x0403_0201);
        assert_eq!(i64_le(&b, 0), 0x0807_0605_0403_0201);
        // Reading at an offset.
        assert_eq!(u16_le(&b, 6), 0x0807);
    }

    #[test]
    fn name_priority_orders_namespaces() {
        // Win32&DOS (3) > Win32 (1) > POSIX (0) > DOS (2) > unknown.
        assert!(name_priority(3) > name_priority(1));
        assert!(name_priority(1) > name_priority(0));
        assert!(name_priority(0) > name_priority(2));
        assert!(name_priority(2) > name_priority(99));
    }

    #[test]
    fn utf16le_decodes_and_is_lossy_on_odd_bytes() {
        // "Hi" in UTF-16LE.
        let bytes = [b'H', 0, b'i', 0];
        assert_eq!(utf16le_to_string(&bytes), "Hi");
        // A trailing odd byte is ignored by chunks_exact (no panic).
        let odd = [b'A', 0, 0xFF];
        assert_eq!(utf16le_to_string(&odd), "A");
    }

    #[test]
    fn parse_data_runs_decodes_one_run() {
        // header 0x21: length field 1 byte, offset field 2 bytes.
        // length = 5 clusters, offset = 0x1000 (relative to prev LCN 0).
        let buf = [0x21u8, 0x05, 0x00, 0x10, 0x00];
        let runs = parse_data_runs(&buf, 0, buf.len());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].length, 5);
        assert_eq!(runs[0].lcn, 0x1000);
    }

    #[test]
    fn parse_data_runs_stops_on_terminator() {
        // A single run then an explicit 0x00 terminator, then junk that must be
        // ignored.
        let buf = [0x11u8, 0x02, 0x04, 0x00, 0xFF, 0xFF];
        let runs = parse_data_runs(&buf, 0, buf.len());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].length, 2);
    }

    #[test]
    fn parse_data_runs_rejects_oversized_run_fields() {
        // A header nibble > 8 (here len=0x0A) is corrupt; the decoder must bail
        // rather than shift by >= 64 in read_signed_le.
        let buf = [0x2Au8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let runs = parse_data_runs(&buf, 0, buf.len());
        assert!(runs.is_empty());
    }

    #[test]
    fn parse_record_never_panics_on_truncated_resident_attr() {
        // A valid FILE header with a resident $STANDARD_INFORMATION attribute whose
        // value-offset points near the record end. Parsing at every truncation
        // length must never read past the record (pre-hardening this aborted when
        // the value-offset field itself fell outside the record).
        let mut rec = vec![0u8; 96];
        rec[0..4].copy_from_slice(b"FILE");
        rec[20..22].copy_from_slice(&56u16.to_le_bytes()); // first attribute at 56
        rec[22] = 0x01; // in-use
        let a = 56;
        rec[a..a + 4].copy_from_slice(&0x10u32.to_le_bytes()); // $STANDARD_INFORMATION
        rec[a + 4..a + 8].copy_from_slice(&8u32.to_le_bytes()); // deliberately tiny alen
        rec[a + 20..a + 22].copy_from_slice(&24u16.to_le_bytes()); // value offset
        for n in 42..=96 {
            let _ = parse_record(&rec[..n], 0, n); // must not panic for any n
        }
    }

    #[test]
    fn extract_mft_data_runs_never_panics_on_truncated_data_attr() {
        // A non-resident $DATA attribute header near the record end; every
        // truncation must be handled without reading past the record.
        let mut rec = vec![0u8; 96];
        rec[20..22].copy_from_slice(&40u16.to_le_bytes()); // first attribute at 40
        let a = 40;
        rec[a..a + 4].copy_from_slice(&0x80u32.to_le_bytes()); // $DATA
        rec[a + 4..a + 8].copy_from_slice(&8u32.to_le_bytes()); // deliberately tiny alen
        rec[a + 8] = 1; // non-resident
        rec[a + 9] = 0; // unnamed
        rec[a + 32..a + 34].copy_from_slice(&48u16.to_le_bytes()); // run-list offset
        for n in 42..=96 {
            let _ = extract_mft_data_runs(&rec[..n]); // must not panic
        }
    }

    #[test]
    fn apply_fixups_restores_sector_tail_bytes() {
        // One 512-byte sector record. USA at offset 48; the per-sector fixup
        // value (bytes 50..52) must be written back to the last 2 bytes of the
        // sector (510..512).
        let mut buf = vec![0u8; 512];
        buf[0..4].copy_from_slice(b"FILE");
        buf[4..6].copy_from_slice(&48u16.to_le_bytes()); // usa_offset
        buf[6..8].copy_from_slice(&2u16.to_le_bytes()); // usa_count (seq + 1 sector)
        buf[50] = 0xAA;
        buf[51] = 0xBB;
        apply_fixups(&mut buf, 0, 512, 512);
        assert_eq!(buf[510], 0xAA);
        assert_eq!(buf[511], 0xBB);
    }

    #[test]
    fn apply_fixups_ignores_non_file_records() {
        let mut buf = vec![0u8; 64];
        buf[0..4].copy_from_slice(b"BAAD");
        let before = buf.clone();
        apply_fixups(&mut buf, 0, 64, 64);
        assert_eq!(buf, before, "non-FILE record must be left untouched");
    }

    fn entry(name: &str, size: i64, is_dir: bool, is_reparse: bool, parent: i64) -> MftEntry {
        MftEntry {
            parent_frn: parent,
            name: name.into(),
            size,
            is_dir,
            is_reparse,
            last_write_ft: 0,
        }
    }

    #[test]
    fn build_subtree_aggregates_and_skips_reparse_dirs() {
        // FRN 5 = root, children: dir "sub"(6), file "f.txt"(7, 100B),
        // junction "link"(8, reparse). Under sub: file "inner.dat"(9, 50B).
        // The junction must be skipped so its (would-be) subtree isn't counted.
        let mut entries: Vec<Option<MftEntry>> = (0..10).map(|_| None).collect();
        entries[5] = Some(entry("root", 0, true, false, 5));
        entries[6] = Some(entry("sub", 0, true, false, 5));
        entries[7] = Some(entry("f.txt", 100, false, false, 5));
        entries[8] = Some(entry("link", 0, true, true, 5));
        entries[9] = Some(entry("inner.dat", 50, false, false, 6));
        // A phantom child under the junction that must NOT be reached.
        // (parent 8 -> would double-count if we recursed into the reparse dir)

        let mut kids: HashMap<i64, Vec<i64>> = HashMap::new();
        kids.insert(5, vec![6, 7, 8]);
        kids.insert(6, vec![9]);

        let mut root = FolderNode {
            full_path: r"C:\".into(),
            name: "C:\\".into(),
            ..Default::default()
        };
        build_subtree(5, &mut root, r"C:\", &entries, &kids, true);

        assert_eq!(
            root.size, 150,
            "100 (f.txt) + 50 (inner.dat); junction excluded"
        );
        assert_eq!(root.file_count, 2);
        assert_eq!(
            root.folder_count, 1,
            "only 'sub'; the reparse dir is skipped"
        );
        // 'sub' is kept, 'link' (reparse) is not.
        let child_names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(child_names, vec!["sub"]);
        assert_eq!(root.files.len(), 1, "f.txt tracked at the root");
    }
}
