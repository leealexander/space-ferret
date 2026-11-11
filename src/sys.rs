//! Platform specifics: how much disk a file really uses, timestamps, volumes,
//! and terminal colour support.

pub struct Volume {
    pub mount: String,
    pub fs: String,
    pub label: String,
    pub total: u64,
    /// bytes available to the current user
    pub free: u64,
}

pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::Volume;
    use std::ffi::c_void;
    use std::fs::Metadata;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::path::Path;
    use std::ptr::null_mut;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        fn GetLogicalDriveStringsW(len: u32, buf: *mut u16) -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs: *mut u16,
            fs_len: u32,
        ) -> i32;
        fn GetVolumePathNameW(path: *const u16, buf: *mut u16, len: u32) -> i32;
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleMode(h: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: *mut c_void, mode: u32) -> i32;
        fn SetErrorMode(mode: u32) -> u32;
    }

    // Cloud-sync placeholders (OneDrive, Google Drive streaming, Dropbox...) report
    // their full logical size but occupy no local disk until hydrated.
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    const CLOUD_ONLY: u32 =
        FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;

    pub const SIZE_BASIS: &str = "logical file size; cloud-only placeholders (OneDrive etc.) are excluded; \
        NTFS hardlinks are counted per link (C:\\Windows\\WinSxS looks bigger than it is)";

    /// (bytes on local disk, bytes that only exist in the cloud)
    #[inline]
    pub fn usage(md: &Metadata) -> (u64, u64) {
        if md.file_attributes() & CLOUD_ONLY != 0 {
            (0, md.len())
        } else {
            (md.len(), 0)
        }
    }

    /// Last-modified time as unix seconds (0 = unknown).
    #[inline]
    pub fn mtime(md: &Metadata) -> i64 {
        let ft = md.last_write_time();
        if ft == 0 {
            0
        } else {
            (ft / 10_000_000) as i64 - 11_644_473_600
        }
    }

    /// Mount points inside a volume are junctions, which the scanner never
    /// follows, so a device id is not needed on Windows.
    #[inline]
    pub fn dev(_: &Metadata) -> u64 {
        0
    }

    #[inline]
    pub fn link_key(_: &Metadata) -> Option<(u64, u64)> {
        None
    }

    fn from_wide(b: &[u16]) -> String {
        let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf16_lossy(&b[..n])
    }

    fn info(root: &str) -> Option<Volume> {
        let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
            return None;
        }
        let mut name = [0u16; 261];
        let mut fs = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                null_mut(),
                null_mut(),
                null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } != 0;
        Some(Volume {
            mount: root.to_string(),
            fs: if ok { from_wide(&fs) } else { String::new() },
            label: if ok { from_wide(&name) } else { String::new() },
            total,
            free: avail,
        })
    }

    pub fn volume_for(path: &Path) -> Option<Volume> {
        let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buf = [0u16; 1024];
        if unsafe { GetVolumePathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } == 0 {
            return None;
        }
        info(&from_wide(&buf))
    }

    pub fn volumes() -> Vec<Volume> {
        // No "insert a disk" dialogs for empty card readers.
        unsafe { SetErrorMode(0x0001 | 0x8000) };
        let mut buf = [0u16; 2048];
        let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        buf[..n.min(buf.len())]
            .split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let root = String::from_utf16_lossy(s);
                let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
                // 0 unknown, 1 no root, 5 CD-ROM
                match unsafe { GetDriveTypeW(w.as_ptr()) } {
                    0 | 1 | 5 => None,
                    _ => info(&root),
                }
            })
            .collect()
    }

    /// Turn on ANSI escape processing for the console (Windows 10+).
    pub fn enable_ansi() -> bool {
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
        unsafe {
            let h = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0u32;
            if h.is_null() || GetConsoleMode(h, &mut mode) == 0 {
                return false;
            }
            mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
                || SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::Volume;
    use std::collections::HashSet;
    use std::ffi::CString;
    use std::fs::Metadata;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    pub const SIZE_BASIS: &str = "allocated disk blocks (same as du); hardlinked files are counted once";

    #[inline]
    pub fn usage(md: &Metadata) -> (u64, u64) {
        (md.blocks() * 512, 0)
    }

    #[inline]
    pub fn mtime(md: &Metadata) -> i64 {
        md.mtime()
    }

    #[inline]
    pub fn dev(md: &Metadata) -> u64 {
        md.dev()
    }

    #[inline]
    pub fn link_key(md: &Metadata) -> Option<(u64, u64)> {
        (md.nlink() > 1).then(|| (md.dev(), md.ino()))
    }

    fn statvfs(path: &str) -> Option<(u64, u64)> {
        let c = CString::new(path).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let f = s.f_frsize as u64;
        Some((s.f_blocks as u64 * f, s.f_bavail as u64 * f))
    }

    /// /proc/mounts escapes spaces etc. as octal: "\040".
    fn unescape(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
                let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
                out.push(v as u8);
                i += 4;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    struct Mount {
        dev: String,
        mnt: String,
        fs: String,
    }

    fn mounts() -> Vec<Mount> {
        std::fs::read_to_string("/proc/self/mounts")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some(Mount { dev: unescape(it.next()?), mnt: unescape(it.next()?), fs: it.next()?.to_string() })
            })
            .collect()
    }

    const REAL_FS: &[&str] = &[
        "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "reiserfs", "bcachefs", "ntfs", "ntfs3",
        "fuseblk", "vfat", "exfat", "hfsplus", "apfs", "9p", "drvfs", "virtiofs", "nfs", "nfs4", "cifs", "smb3",
        "overlay",
    ];

    pub fn volumes() -> Vec<Volume> {
        let ms = mounts();
        if ms.is_empty() {
            return statvfs("/")
                .map(|(total, free)| {
                    vec![Volume { mount: "/".into(), fs: String::new(), label: String::new(), total, free }]
                })
                .unwrap_or_default();
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for m in ms {
            // container / WSL plumbing, not places a user stores data
            let plumbing = (m.fs == "overlay" && m.mnt != "/")
                || ["/usr/", "/snap/", "/proc", "/sys", "/var/lib/docker", "/var/lib/containers"]
                    .iter()
                    .any(|p| m.mnt.starts_with(p))
                || (m.mnt.starts_with("/run/") && !m.mnt.starts_with("/run/media/"));
            if plumbing || !REAL_FS.contains(&m.fs.as_str()) || !seen.insert(m.dev.clone()) {
                continue;
            }
            if let Some((total, free)) = statvfs(&m.mnt) {
                if total > 0 {
                    out.push(Volume { mount: m.mnt, fs: m.fs, label: m.dev, total, free });
                }
            }
        }
        out
    }

    pub fn volume_for(path: &Path) -> Option<Volume> {
        let p = path.to_string_lossy().to_string();
        let (total, free) = statvfs(&p)?;
        let best = mounts()
            .into_iter()
            .filter(|m| m.mnt == "/" || p == m.mnt || p.starts_with(&format!("{}/", m.mnt)))
            .max_by_key(|m| m.mnt.len());
        Some(match best {
            Some(m) => Volume { mount: m.mnt, fs: m.fs, label: m.dev, total, free },
            None => Volume { mount: "/".into(), fs: String::new(), label: String::new(), total, free },
        })
    }

    pub fn enable_ansi() -> bool {
        true
    }
}
