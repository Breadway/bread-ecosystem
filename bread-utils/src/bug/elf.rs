//! Just enough ELF reading to find a module's GNU build-id and whether it is
//! position independent. Pure `std`; never panics on malformed input.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const PT_NOTE: u32 = 4;
const NT_GNU_BUILD_ID: u32 = 3;
const MAX_NOTE_BYTES: u64 = 64 * 1024;
const MAX_PHDRS: u16 = 256;

/// What the crash reporter needs to know about a loaded module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfInfo {
    /// Lowercase hex GNU build-id, if the module has one.
    pub build_id: Option<String>,
    /// `ET_DYN` (PIE or shared object): addresses are relative to the load
    /// base. `ET_EXEC` addresses are absolute.
    pub is_dyn: bool,
}

fn le_u16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse the notes in a `PT_NOTE` segment, returning the GNU build-id.
pub fn build_id_from_notes(mut notes: &[u8]) -> Option<String> {
    while notes.len() >= 12 {
        let namesz = le_u32(&notes[0..4]) as usize;
        let descsz = le_u32(&notes[4..8]) as usize;
        let kind = le_u32(&notes[8..12]);
        let name_end = 12usize.checked_add(namesz.checked_add(3)? & !3)?;
        let desc_end = name_end.checked_add(descsz.checked_add(3)? & !3)?;
        if desc_end > notes.len() {
            return None;
        }
        let name = &notes[12..12 + namesz.min(notes.len() - 12)];
        if kind == NT_GNU_BUILD_ID && name.starts_with(b"GNU\0") && descsz > 0 {
            return Some(hex(&notes[name_end..name_end + descsz]));
        }
        notes = &notes[desc_end..];
    }
    None
}

/// Read build-id and type from an ELF file; `None` if it is not a
/// little-endian 64-bit ELF.
pub fn read(path: &Path) -> Option<ElfInfo> {
    let mut f = File::open(path).ok()?;
    let mut hdr = [0u8; 64];
    f.read_exact(&mut hdr).ok()?;
    if &hdr[0..4] != b"\x7fELF" || hdr[4] != 2 || hdr[5] != 1 {
        return None;
    }
    let is_dyn = le_u16(&hdr[16..18]) == 3;
    let phoff = le_u64(&hdr[32..40]);
    let phentsize = u64::from(le_u16(&hdr[54..56]));
    let phnum = le_u16(&hdr[56..58]).min(MAX_PHDRS);
    let mut build_id = None;
    if phentsize >= 56 {
        for i in 0..u64::from(phnum) {
            let mut ph = [0u8; 56];
            if f.seek(SeekFrom::Start(phoff.checked_add(i * phentsize)?))
                .is_err()
                || f.read_exact(&mut ph).is_err()
            {
                break;
            }
            if le_u32(&ph[0..4]) != PT_NOTE {
                continue;
            }
            let (off, size) = (le_u64(&ph[8..16]), le_u64(&ph[32..40]));
            if size == 0 || size > MAX_NOTE_BYTES {
                continue;
            }
            let mut notes = vec![0u8; size as usize];
            if f.seek(SeekFrom::Start(off)).is_ok() && f.read_exact(&mut notes).is_ok() {
                if let Some(id) = build_id_from_notes(&notes) {
                    build_id = Some(id);
                    break;
                }
            }
        }
    }
    Some(ElfInfo { build_id, is_dyn })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(name: &[u8], kind: u32, desc: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend((name.len() as u32).to_le_bytes());
        v.extend((desc.len() as u32).to_le_bytes());
        v.extend(kind.to_le_bytes());
        v.extend(name);
        while v.len() % 4 != 0 {
            v.push(0);
        }
        v.extend(desc);
        while v.len() % 4 != 0 {
            v.push(0);
        }
        v
    }

    #[test]
    fn finds_the_gnu_build_id_among_other_notes() {
        let mut notes = note(b"GNU\0", 1, &[1, 2, 3, 4]);
        notes.extend(note(
            b"GNU\0",
            NT_GNU_BUILD_ID,
            &[0xde, 0xad, 0xbe, 0xef, 0x01],
        ));
        assert_eq!(build_id_from_notes(&notes).as_deref(), Some("deadbeef01"));
    }

    #[test]
    fn ignores_wrong_vendor_and_truncated_notes() {
        assert_eq!(
            build_id_from_notes(&note(b"XYZ\0", NT_GNU_BUILD_ID, &[1, 2])),
            None
        );
        let mut n = note(b"GNU\0", NT_GNU_BUILD_ID, &[1, 2, 3, 4]);
        n.truncate(n.len() - 2);
        assert_eq!(build_id_from_notes(&n), None);
        assert_eq!(build_id_from_notes(&[]), None);
        // sizes that would overflow must not panic
        let mut evil = vec![0xff; 12];
        evil.extend([0u8; 8]);
        assert_eq!(build_id_from_notes(&evil), None);
    }

    #[test]
    fn reads_the_running_test_binary() {
        let exe = std::env::current_exe().unwrap();
        let info = read(&exe).expect("test binary is an ELF");
        assert!(info.is_dyn, "Rust test binaries are PIE");
        if let Some(id) = info.build_id {
            assert!(id.len() >= 16 && id.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn non_elf_and_missing_files_are_none() {
        assert!(read(Path::new("/definitely/not/here")).is_none());
        let dir = std::env::temp_dir().join(format!("elf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x");
        std::fs::write(&p, b"not an elf at all, just text").unwrap();
        assert!(read(&p).is_none());
        std::fs::write(&p, b"\x7fELF").unwrap();
        assert!(read(&p).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
