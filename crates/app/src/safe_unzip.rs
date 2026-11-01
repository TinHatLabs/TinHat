//! The Windows self-updater's unzip (deps round 2026-10-31, zip 2.4.2 → 9.0.5 for the Dependabot "zip cloaking" /
//! symlink / AES-HMAC advisories). platform.rs `install_update_zip` only ever opens bytes whose sha256 update.rs has
//! already matched against the release-key-signed feed, so only our own zips reach this code; it is still written so
//! that a hostile archive cannot write outside `dest` or replace one entry with another:
//!
//! - every name is checked here, not trusted to the crate. `/` and `\` both separate folders (package.ps1's
//!   `Compress-Archive` on Windows PowerShell 5.1 writes `lib\gstreamer-1.0\x.dll`: 2 291 of 12.1.4's 2 403 names), so
//!   a name is split on both and then refused if it is absolute or UNC, has a drive letter, a `..` or `.` or empty
//!   component, a NUL/control character, a `:` (drive or NTFS stream), a Windows-reserved character (`<>"|?*`), a
//!   trailing dot or space, a reserved device name (`CON`, `NUL`, `COM1`, …), an 8.3 short-name shape (`LONGDI~1`,
//!   which NTFS may resolve to another folder), or more than [`MAX_DEPTH`] components;
//! - only plain files and directories: symlinks, other special entries and encrypted entries are refused;
//! - the archive's headers must agree before anything is written (zip 9's `validate_header_consistency`: no
//!   duplicate names in the central directory, every local header naming the same file as its central entry, no
//!   overlapping entries — the "cloaking" shapes);
//! - one entry per path (ASCII-case-insensitive, as NTFS is for our names), and every file is created with
//!   `create_new`, so no entry can overwrite another;
//! - every folder on the way to an entry, `dest` included, must be a real folder (not a symlink or junction) after it
//!   is created; staging is a fresh folder in the user's own data folder, so only a process already running as that
//!   user could race a link in, and such a process can replace `current\tinhat.exe` directly anyway;
//! - a folder entry carries no data and a file entry exactly the size its header declares;
//! - at most [`MAX_ENTRIES`] filesystem objects (entries plus the folders they imply) and [`MAX_TOTAL_BYTES`] written
//!   in total (counted while writing, not taken from the headers); the crate checks each entry's CRC-32 as it is read.
//!   The archive itself is already in memory (the downloaded, sha256-checked bundle), which bounds what the zip
//!   crate parses.
//!
//! Any refusal fails the whole update; the caller deletes the staging folder and the installed bundle is untouched.

use std::collections::HashSet;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};

// The limits are also a way to strand updates: a bundle that outgrows one is refused by every install that has this
// code. So each has >= 4x headroom over the real bundle (12.1.5: 2 381 files + 109 folders = 2 490 objects, 761.8 MiB
// unpacked, largest entry Qt6WebEngineCore.dll 147.3 MiB, compression 2.40x, depth 8), and `real_bundle` (run on
// every release zip) fails once a bundle passes HALF of any limit, long before an install would refuse it. Raising
// a limit only helps installs that already carry the raised value: raise it one release ahead.
/// Files and folders, explicit or implied (12.1.5: 2 490, i.e. 20x headroom).
pub(crate) const MAX_ENTRIES: usize = 50_000;
/// Path components of one entry (12.1.5: 8, i.e. 8x headroom).
pub(crate) const MAX_DEPTH: usize = 64;
/// Uncompressed bytes in total (12.1.5: 761.8 MiB, i.e. 10.7x headroom). There is no per-entry or compression-ratio
/// limit: this total bounds both.
pub(crate) const MAX_TOTAL_BYTES: u64 = 8 << 30;

/// Extract `reader` (a zip) into the existing directory `dest`. Returns the number of files written.
pub(crate) fn extract_zip<R: Read + Seek>(reader: R, dest: &Path) -> Result<usize, String> {
    extract_zip_limited(reader, dest, MAX_ENTRIES, MAX_TOTAL_BYTES)
}

fn extract_zip_limited<R: Read + Seek>(reader: R, dest: &Path, max_entries: usize, max_bytes: u64) -> Result<usize, String> {
    let meta = std::fs::symlink_metadata(dest).map_err(|e| format!("unzip target {}: {e}", dest.display()))?;
    if !meta.is_dir() {
        return Err(format!("unzip target {} is not a plain directory", dest.display()));
    }
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("open zip: {e}"))?;
    if archive.len() > max_entries {
        return Err(format!("zip has {} entries (limit {max_entries})", archive.len()));
    }
    archive.validate_header_consistency().map_err(|e| format!("zip headers disagree: {e}"))?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut folders: HashSet<String> = HashSet::new();
    let mut written: u64 = 0;
    let mut files = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("zip entry {i}: {e}"))?;
        let name = entry.name().map_err(|e| format!("zip entry {i}: name: {e}"))?.into_owned();
        let (rel, key) = safe_relative_path(&name).map_err(|why| format!("refusing zip entry {name:?}: {why}"))?;
        let is_dir = entry.is_dir() || name.ends_with('/') || name.ends_with('\\');
        if entry.encrypted() {
            return Err(format!("refusing zip entry {name:?}: encrypted"));
        }
        if entry.is_symlink() {
            return Err(format!("refusing zip entry {name:?}: symlink"));
        }
        if let Some(kind) = entry.unix_mode().map(|m| m & 0o170000) {
            if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                return Err(format!("refusing zip entry {name:?}: special file (mode {kind:o})"));
            }
        }
        if !seen.insert(key.clone()) {
            return Err(format!("refusing zip entry {name:?}: a second entry for the same path"));
        }
        // every folder this entry implies (and the entry itself if it is one) counts against the object budget
        let mut prefix = String::new();
        let parts: Vec<&str> = key.split('/').collect();
        let implied = if is_dir { parts.len() } else { parts.len() - 1 };
        for part in &parts[..implied] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            folders.insert(prefix.clone());
        }
        if files + usize::from(!is_dir) + folders.len() > max_entries {
            return Err(format!("zip unpacks to more than {max_entries} files and folders"));
        }
        let out = dest.join(&rel);
        if is_dir {
            if entry.size() != 0 {
                return Err(format!("refusing zip entry {name:?}: a folder entry with content"));
            }
            std::fs::create_dir_all(&out).map_err(|e| format!("mkdir {}: {e}", out.display()))?;
            check_real_folders(dest, &rel)?;
            continue;
        }
        if let Some(parent) = rel.parent() {
            let parent_out = dest.join(parent);
            std::fs::create_dir_all(&parent_out).map_err(|e| format!("mkdir {}: {e}", parent_out.display()))?;
            check_real_folders(dest, parent)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out)
            .map_err(|e| format!("create {}: {e}", out.display()))?;
        let budget = max_bytes - written;
        let n = std::io::copy(&mut (&mut entry).take(budget + 1), &mut f)
            .map_err(|e| format!("write {}: {e}", out.display()))?;
        if n > budget {
            return Err(format!("zip unpacks to more than {max_bytes} bytes"));
        }
        if n != entry.size() {
            return Err(format!("refusing zip entry {name:?}: {n} bytes, its header says {}", entry.size()));
        }
        written += n;
        files += 1;
    }
    Ok(files)
}

/// `dest` and each folder of `rel` below it must be a real folder: `symlink_metadata` reports a symlink or a junction
/// (a name-surrogate reparse point) as not-a-folder.
fn check_real_folders(dest: &Path, rel: &Path) -> Result<(), String> {
    let mut p = dest.to_path_buf();
    let check = |p: &Path| match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(format!("refusing to write through {}: not a plain folder", p.display())),
        Err(e) => Err(format!("{}: {e}", p.display())),
    };
    check(&p)?;
    for c in rel.components() {
        p.push(c);
        check(&p)?;
    }
    Ok(())
}

/// `name` (a zip entry name, `/`- or `\`-separated) as a relative path of plain components, plus its duplicate key
/// (lower-case, `/`-joined). One trailing separator (a directory entry) is allowed; everything listed in the module doc
/// is not.
pub(crate) fn safe_relative_path(name: &str) -> Result<(PathBuf, String), &'static str> {
    if name.is_empty() {
        return Err("empty name");
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("control character");
    }
    if name.contains(':') {
        return Err("colon (drive letter or stream)");
    }
    if name.chars().any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*')) {
        return Err("character Windows does not allow");
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err("absolute path");
    }
    let trimmed = name.strip_suffix(['/', '\\']).unwrap_or(name);
    let mut rel = PathBuf::new();
    let mut key = String::new();
    let parts: Vec<&str> = trimmed.split(['/', '\\']).collect();
    if parts.len() > MAX_DEPTH {
        return Err("too many folders deep");
    }
    for part in parts {
        if part.is_empty() {
            return Err("empty path component");
        }
        if part == "." || part == ".." {
            return Err("dot component");
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err("trailing dot or space");
        }
        if is_reserved_device_name(part) {
            return Err("reserved device name");
        }
        if looks_like_short_name(part) {
            return Err("8.3 short-name shape");
        }
        rel.push(part);
        if !key.is_empty() {
            key.push('/');
        }
        // ASCII only: Unicode lower-casing is not NTFS's case rule (it would join `İ` and `i̇`); a non-ASCII case
        // twin is still stopped by `create_new` when it names the same file.
        key.push_str(&part.to_ascii_lowercase());
    }
    // Belt and braces: whatever the platform's path parser makes of it must be plain components only.
    if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("not a plain relative path");
    }
    Ok((rel, key))
}

/// `LONGDI~1`-style names: NTFS may answer one with another (long-named) folder or file.
fn looks_like_short_name(part: &str) -> bool {
    let b = part.as_bytes();
    b.iter().enumerate().any(|(i, &c)| c == b'~' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
}

fn is_reserved_device_name(part: &str) -> bool {
    // Windows ignores the extension and trailing spaces: `nul.txt` and `COM1 .dll` still name the device.
    let stem = part.split('.').next().unwrap_or(part).trim_end().to_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let rest: Vec<char> = chars.collect();
    (prefix == "COM" || prefix == "LPT") && rest.len() == 1 && matches!(rest[0], '0'..='9' | '¹' | '²' | '³')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tinhat-safe-unzip-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn build(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            if name.ends_with('/') {
                w.add_directory(*name, SimpleFileOptions::default()).unwrap();
            } else {
                w.start_file(*name, SimpleFileOptions::default()).unwrap();
                w.write_all(data).unwrap();
            }
        }
        w.finish().unwrap().into_inner()
    }

    /// A zip where the writer is told the plain name, then the raw name bytes in both headers are patched to `raw`
    /// (same length): the only way to get names the writer itself refuses into a test archive.
    fn build_raw_name(plain: &str, raw: &str, data: &[u8]) -> Vec<u8> {
        assert_eq!(plain.len(), raw.len());
        let mut z = build(&[(plain, data)]);
        let mut hits = 0;
        let mut i = 0;
        while i + plain.len() <= z.len() {
            if &z[i..i + plain.len()] == plain.as_bytes() {
                z[i..i + plain.len()].copy_from_slice(raw.as_bytes());
                hits += 1;
                i += plain.len();
            } else {
                i += 1;
            }
        }
        assert_eq!(hits, 2, "name in the local header and the central directory");
        z
    }

    fn listing(root: &Path) -> Vec<String> {
        fn walk(root: &Path, p: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(p).unwrap() {
                let e = e.unwrap();
                let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                if e.file_type().unwrap().is_dir() {
                    out.push(format!("{rel}/"));
                    walk(root, &e.path(), out);
                } else {
                    out.push(rel);
                }
            }
        }
        let mut v = Vec::new();
        walk(root, root, &mut v);
        v.sort();
        v
    }

    #[test]
    fn a_bundle_shaped_zip_extracts() {
        let z = build(&[
            ("tinhat.exe", b"MZ"),
            ("VERSION", b"12.1.6\n"),
            ("lib/", b""),
            ("lib/gstreamer-1.0/gstcoreelements.dll", b"x"),
            ("qml/QtQuick/Controls/qmldir", b"module"),
        ]);
        let d = tmpdir("ok");
        assert_eq!(extract_zip(Cursor::new(z), &d).unwrap(), 4);
        assert_eq!(std::fs::read_to_string(d.join("VERSION")).unwrap(), "12.1.6\n");
        assert_eq!(
            listing(&d),
            ["VERSION", "lib/", "lib/gstreamer-1.0/", "lib/gstreamer-1.0/gstcoreelements.dll", "qml/",
             "qml/QtQuick/", "qml/QtQuick/Controls/", "qml/QtQuick/Controls/qmldir", "tinhat.exe"]
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_compress_archive_zip_with_backslash_names_extracts_into_folders() {
        // the shape Windows PowerShell 5.1's Compress-Archive writes (12.1.4's real bundle): `\` separators, folder
        // entries ending in `\`
        let z = build(&[
            ("tinhat.exe", b"MZ"),
            ("iconpack\\theme\\", b""),
            ("iconpack\\theme\\x.svg", b"<svg/>"),
            ("lib\\gstreamer-1.0\\gstcoreelements.dll", b"x"),
        ]);
        let d = tmpdir("bs");
        assert_eq!(extract_zip(Cursor::new(z), &d).unwrap(), 3);
        assert_eq!(std::fs::read(d.join("iconpack").join("theme").join("x.svg")).unwrap(), b"<svg/>");
        assert!(d.join("lib").join("gstreamer-1.0").join("gstcoreelements.dll").is_file());
        std::fs::remove_dir_all(&d).unwrap();
        // `lib/a` and `lib\a` are one path
        let d = tmpdir("bsdup");
        let z = build(&[("lib/a.dll", b"one"), ("lib\\a.dll", b"two")]);
        assert!(extract_zip(Cursor::new(z), &d).unwrap_err().contains("second entry"));
        assert_eq!(std::fs::read(d.join("lib").join("a.dll")).unwrap(), b"one");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn names_that_could_leave_the_folder_are_refused() {
        for bad in [
            "", "/etc/passwd", "../x", "a/../../x", "a/./b", "./a", "a//b", "C:/x", "c:x", "a/b:stream",
            "\\\\server\\share\\x", "\\x", "a\\..\\x", "..\\x", "a\\\\b", "a/\\b", "lib\\\\", "a\0b", "a\nb", "x.", "dir /x", "a/CON", "nul.txt",
            "COM1.dll", "lpt9", "a?b", "a*b", "a|b", "a<b", "a>b", "a\"b", "conin$", "/",
        ] {
            assert!(safe_relative_path(bad).is_err(), "{bad:?} must be refused");
        }
        for good in ["tinhat.exe", "lib/", "a/b/c.dll", "Qt6Core.dll", "share/glib-2.0/schemas/x.xml", "...a", "con-x/y",
                     "COM10.txt", "nullsink.dll", "auxiliary",
                     "lib\\gstreamer-1.0\\gstcoreelements.dll", "iconpack\\theme\\", "a/b\\c"] {
            assert!(safe_relative_path(good).is_ok(), "{good:?} must be allowed");
        }
        assert_eq!(safe_relative_path("Lib/Qt6Core.DLL").unwrap().1, "lib/qt6core.dll");
        assert_eq!(safe_relative_path("Lib\\Qt6Core.DLL").unwrap().1, "lib/qt6core.dll");
        for bad in ["LONGDI~1/b.dll", "a/PROGRA~12", "x~0"] {
            assert!(safe_relative_path(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(safe_relative_path("a~b/c~").is_ok());
        let deep = vec!["d"; MAX_DEPTH].join("/");
        assert!(safe_relative_path(&deep).is_ok());
        assert!(safe_relative_path(&format!("{deep}/x")).is_err());
        // distinct NTFS names that Unicode lower-casing would join
        assert_ne!(safe_relative_path("\u{130}.txt").unwrap().1, safe_relative_path("i\u{307}.txt").unwrap().1);
        assert_eq!(safe_relative_path("a\\b\\c.dll").unwrap().0, PathBuf::from("a").join("b").join("c.dll"));
    }

    #[test]
    fn traversal_and_absolute_entries_fail_and_write_nothing_outside() {
        let outer = tmpdir("trav");
        let d = outer.join("stage");
        std::fs::create_dir_all(&d).unwrap();
        for (plain, raw) in [("aa/xx", "../xx"), ("abcdefg", "/tmp/xx"), ("abxx", "C:xx"), ("ab/xxx", "..\\xxx")] {
            let z = build_raw_name(plain, raw, b"pwned");
            let err = extract_zip(Cursor::new(z), &d).unwrap_err();
            assert!(err.starts_with("refusing zip entry"), "{raw:?}: {err}");
        }
        assert!(!outer.join("xx").exists());
        assert!(listing(&d).is_empty(), "{:?}", listing(&d));
        std::fs::remove_dir_all(&outer).unwrap();
    }

    #[test]
    fn a_second_entry_for_the_same_path_is_refused_and_never_overwrites() {
        let d = tmpdir("dup");
        // same name twice (the writer refuses, so patch the second name in)
        let z = {
            let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
            w.start_file("tinhat.exe", SimpleFileOptions::default()).unwrap();
            w.write_all(b"first").unwrap();
            w.start_file("tinhat.exf", SimpleFileOptions::default()).unwrap();
            w.write_all(b"second").unwrap();
            let mut z = w.finish().unwrap().into_inner();
            let pat = b"tinhat.exf";
            let mut i = 0;
            while i + pat.len() <= z.len() {
                if &z[i..i + pat.len()] == pat {
                    z[i..i + pat.len()].copy_from_slice(b"tinhat.exe");
                }
                i += 1;
            }
            z
        };
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("headers disagree") || err.contains("open zip"), "{err}");
        if let Ok(s) = std::fs::read(d.join("tinhat.exe")) {
            assert_eq!(s, b"first");
        }
        std::fs::remove_dir_all(&d).unwrap();

        // case-insensitive twin (NTFS would open the same file)
        let d = tmpdir("dupcase");
        let z = build(&[("Lib/a.dll", b"one"), ("lib/A.DLL", b"two")]);
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("second entry"), "{err}");
        assert_eq!(std::fs::read(d.join("Lib/a.dll")).unwrap(), b"one");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn symlinks_and_special_files_are_refused() {
        let d = tmpdir("link");
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.add_symlink("current", "../../outside", SimpleFileOptions::default()).unwrap();
        let z = w.finish().unwrap().into_inner();
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("symlink"), "{err}");
        assert!(listing(&d).is_empty());

        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let unix = SimpleFileOptions::default().system(zip::System::Unix);
        w.start_file("fifo", unix.external_attributes(0o010644 << 16)).unwrap();
        let z = w.finish().unwrap().into_inner();
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("special file") || err.contains("symlink"), "{err}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn implied_folders_count_against_the_object_limit() {
        let d = tmpdir("nodes");
        let z = build(&[("a/b/c/1", b""), ("a/b/c/2", b"")]);
        // 2 files + a, a/b, a/b/c
        assert_eq!(extract_zip_limited(Cursor::new(z.clone()), &d, 5, 1 << 20).unwrap(), 2);
        std::fs::remove_dir_all(&d).unwrap();
        let d = tmpdir("nodes2");
        assert!(extract_zip_limited(Cursor::new(z), &d, 4, 1 << 20).unwrap_err().contains("files and folders"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_entry_with_content_is_refused() {
        let d = tmpdir("dirdata");
        // written as a file whose name ends in a separator (the writer's add_directory would carry no data)
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("lib\\", SimpleFileOptions::default()).unwrap();
        w.write_all(b"hidden").unwrap();
        let z = w.finish().unwrap().into_inner();
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("folder entry with content"), "{err}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_folder_inside_the_target_is_not_written_through() {
        let outer = tmpdir("parentlink");
        let d = outer.join("stage");
        let elsewhere = outer.join("elsewhere");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, d.join("lib")).unwrap();
        let z = build(&[("lib/new.dll", b"x")]);
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("not a plain folder"), "{err}");
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing written through the link");
        // and a linked target itself
        let z = build(&[("a", b"x")]);
        assert!(extract_zip(Cursor::new(z), &d.join("lib")).is_err());
        std::fs::remove_dir_all(&outer).unwrap();
    }

    #[test]
    fn a_file_where_a_folder_is_expected_fails_closed() {
        let d = tmpdir("filedir");
        let z = build(&[("lib", b"file"), ("lib/x.dll", b"x")]);
        assert!(extract_zip(Cursor::new(z), &d).is_err());
        assert_eq!(std::fs::read(d.join("lib")).unwrap(), b"file");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_size_and_count_limits_hold() {
        let d = tmpdir("limits");
        let z = build(&[("a", &[0u8; 100]), ("b", &[0u8; 100])]);
        let err = extract_zip_limited(Cursor::new(z.clone()), &d, 1, 1 << 20).unwrap_err();
        assert!(err.contains("entries"), "{err}");
        let err = extract_zip_limited(Cursor::new(z.clone()), &d, 10, 150).unwrap_err();
        assert!(err.contains("more than 150 bytes"), "{err}");
        std::fs::remove_dir_all(&d).unwrap();
        let d = tmpdir("limits2");
        assert_eq!(extract_zip_limited(Cursor::new(z), &d, 2, 200).unwrap(), 2);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_local_header_that_names_another_file_is_refused() {
        // "zip cloaking": the central directory says VERSION, the local header (what gets read) says something else
        let d = tmpdir("cloak");
        let mut z = build(&[("VERSION", b"12.1.6")]);
        let first = z.windows(7).position(|w| w == b"VERSION").unwrap();
        z[first..first + 7].copy_from_slice(b"VERSIOX");
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("headers disagree"), "{err}");
        assert!(listing(&d).is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_corrupted_entry_fails_its_crc() {
        let d = tmpdir("crc");
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        w.start_file("VERSION", stored).unwrap();
        w.write_all(b"12.1.6 version file payload").unwrap();
        let mut z = w.finish().unwrap().into_inner();
        let pos = z.windows(7).position(|w| w == b"payload").unwrap();
        z[pos] ^= 0x01;
        let err = extract_zip(Cursor::new(z), &d).unwrap_err();
        assert!(err.contains("write") || err.contains("CRC") || err.contains("checksum"), "{err}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_target_must_be_a_plain_existing_folder() {
        let d = tmpdir("target");
        let z = build(&[("a", b"x")]);
        assert!(extract_zip(Cursor::new(z.clone()), &d.join("missing")).is_err());
        std::fs::write(d.join("file"), b"").unwrap();
        assert!(extract_zip(Cursor::new(z), &d.join("file")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The real release bundle: `TINHAT_SAFE_UNZIP_BUNDLE=/path/tinhat-<ver>-win64.zip cargo test --release -p tinhat-app
    /// safe_unzip::tests::real_bundle -- --ignored` (a release-train step, BUILDING.md). It extracts, and it FAILS once
    /// the bundle uses more than half of any limit, so a limit is raised a release before an install would refuse.
    #[test]
    #[ignore]
    fn real_bundle() {
        let p = std::env::var_os("TINHAT_SAFE_UNZIP_BUNDLE").expect("set TINHAT_SAFE_UNZIP_BUNDLE to a release zip");
        let d = tmpdir("real");
        // half of every limit, measured from the archive itself
        let mut a = zip::ZipArchive::new(std::fs::File::open(&p).unwrap()).unwrap();
        let (mut objects, mut bytes, mut depth) = (HashSet::new(), 0u64, 0usize);
        for i in 0..a.len() {
            let e = a.by_index(i).unwrap();
            let name = e.name().unwrap().into_owned();
            let (_, key) = safe_relative_path(&name).unwrap();
            let parts: Vec<&str> = key.split('/').collect();
            depth = depth.max(parts.len());
            for k in 1..=parts.len() {
                objects.insert(parts[..k].join("/"));
            }
            bytes += e.size();
        }
        eprintln!(
            "real bundle: {} objects (limit {MAX_ENTRIES}), {bytes} bytes (limit {MAX_TOTAL_BYTES}), depth {depth} (limit {MAX_DEPTH})",
            objects.len()
        );
        assert!(objects.len() * 2 <= MAX_ENTRIES, "bundle uses over half of MAX_ENTRIES: raise it (one release ahead)");
        assert!(bytes * 2 <= MAX_TOTAL_BYTES, "bundle uses over half of MAX_TOTAL_BYTES: raise it (one release ahead)");
        assert!(depth * 2 <= MAX_DEPTH, "bundle uses over half of MAX_DEPTH: raise it (one release ahead)");
        let n = extract_zip(std::fs::File::open(&p).unwrap(), &d).unwrap();
        let has_exe = d.join("tinhat.exe").is_file()
            || std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).any(|e| e.path().join("tinhat.exe").is_file());
        eprintln!("real bundle: {n} files, tinhat.exe present: {has_exe}");
        assert!(has_exe);
        assert!(d.join("lib").is_dir() || std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok()).any(|e| e.path().join("lib").is_dir()),
                "folders came out as folders");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
