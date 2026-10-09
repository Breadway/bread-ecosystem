//! Mapping an instruction pointer to `(module, offset)` using
//! `/proc/self/maps`, so a stripped binary's stack can be symbolicated later
//! from the module's build-id and the module-relative address.

use std::path::Path;

use super::elf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub path: String,
    pub build_id: Option<String>,
    /// Lowest address this file is mapped at.
    pub base: u64,
    /// ET_DYN modules report `ip - base`; ET_EXEC report `ip`.
    pub is_dyn: bool,
}

/// An executable mapping and the module it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub module: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Layout {
    pub modules: Vec<Module>,
    pub regions: Vec<Region>,
}

/// Parse the text of a maps file. `elf_info` supplies build-id and type for
/// a path (injected so tests need no real files).
pub fn parse(text: &str, elf_info: impl Fn(&Path) -> Option<elf::ElfInfo>) -> Layout {
    struct Line<'a> {
        start: u64,
        end: u64,
        exec: bool,
        path: &'a str,
    }
    let lines: Vec<Line<'_>> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(6, char::is_whitespace).filter(|p| !p.is_empty());
            let range = it.next()?;
            let perms = it.next()?;
            let path = l.get(l.find('/')?..)?.trim_end();
            let (s, e) = range.split_once('-')?;
            Some(Line {
                start: u64::from_str_radix(s, 16).ok()?,
                end: u64::from_str_radix(e, 16).ok()?,
                exec: perms.contains('x'),
                path,
            })
        })
        .collect();

    let mut layout = Layout::default();
    for l in &lines {
        // Pseudo-files like "/memfd:..." and deleted files are not modules.
        if l.path.starts_with("/memfd:") || l.path.ends_with(" (deleted)") {
            continue;
        }
        let idx = match layout.modules.iter().position(|m| m.path == l.path) {
            Some(i) => {
                layout.modules[i].base = layout.modules[i].base.min(l.start);
                i
            }
            None => {
                let info = elf_info(Path::new(l.path));
                layout.modules.push(Module {
                    path: l.path.to_string(),
                    build_id: info.as_ref().and_then(|i| i.build_id.clone()),
                    base: l.start,
                    is_dyn: info.is_none_or(|i| i.is_dyn),
                });
                layout.modules.len() - 1
            }
        };
        if l.exec {
            layout.regions.push(Region {
                start: l.start,
                end: l.end,
                module: idx,
            });
        }
    }
    layout
}

impl Layout {
    /// Read and parse this process's own maps.
    pub fn current() -> Self {
        std::fs::read_to_string("/proc/self/maps")
            .map(|t| parse(&t, elf::read))
            .unwrap_or_default()
    }

    /// The module containing `ip` and the module-relative offset.
    pub fn locate(&self, ip: u64) -> Option<(&Module, u64)> {
        let r = self.regions.iter().find(|r| ip >= r.start && ip < r.end)?;
        let m = &self.modules[r.module];
        let off = if m.is_dyn { ip - m.base } else { ip };
        Some((m, off))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPS: &str = "\
55d0c0000000-55d0c0010000 r--p 00000000 fd:01 1234 /usr/bin/breadclip
55d0c0010000-55d0c0100000 r-xp 00010000 fd:01 1234 /usr/bin/breadclip
55d0c0100000-55d0c0110000 rw-p 00100000 fd:01 1234 /usr/bin/breadclip
7f0000000000-7f0000020000 r--p 00000000 fd:01 99 /usr/lib/libc.so.6
7f0000020000-7f0000150000 r-xp 00020000 fd:01 99 /usr/lib/libc.so.6
7ffd00000000-7ffd00021000 rw-p 00000000 00:00 0 [stack]
7ffd00100000-7ffd00102000 r-xp 00000000 00:00 0 [vdso]
7f0000200000-7f0000210000 r-xp 00000000 fd:01 5 /tmp/x (deleted)
7f0000300000-7f0000310000 r-xp 00000000 00:00 0 /memfd:jit (deleted)
400000-401000 r-xp 00000000 fd:01 7 /opt/fixed
";

    fn info(p: &Path) -> Option<elf::ElfInfo> {
        match p.to_str()? {
            "/usr/bin/breadclip" => Some(elf::ElfInfo {
                build_id: Some("aabbccdd11".into()),
                is_dyn: true,
            }),
            "/usr/lib/libc.so.6" => Some(elf::ElfInfo {
                build_id: None,
                is_dyn: true,
            }),
            "/opt/fixed" => Some(elf::ElfInfo {
                build_id: Some("ff00ff00".into()),
                is_dyn: false,
            }),
            _ => None,
        }
    }

    #[test]
    fn locates_ips_in_pie_modules_relative_to_their_base() {
        let l = parse(MAPS, info);
        let (m, off) = l.locate(0x55d0c0010000 + 0x1234).unwrap();
        assert_eq!(m.path, "/usr/bin/breadclip");
        assert_eq!(m.build_id.as_deref(), Some("aabbccdd11"));
        // base is the lowest mapping (file offset 0), not the exec mapping
        assert_eq!(off, 0x10000 + 0x1234);
        let (m, off) = l.locate(0x7f0000020010).unwrap();
        assert_eq!(
            (m.path.as_str(), off, m.build_id.clone()),
            ("/usr/lib/libc.so.6", 0x20010, None)
        );
    }

    #[test]
    fn non_pie_executables_report_absolute_addresses() {
        let l = parse(MAPS, info);
        let (m, off) = l.locate(0x400800).unwrap();
        assert_eq!((m.path.as_str(), off), ("/opt/fixed", 0x400800));
    }

    #[test]
    fn data_stack_vdso_deleted_and_memfd_are_not_modules() {
        let l = parse(MAPS, info);
        assert!(l.locate(0x55d0c0100500).is_none(), "rw- data mapping");
        assert!(l.locate(0x7ffd00000100).is_none(), "stack");
        assert!(l.locate(0x7ffd00100100).is_none(), "vdso has no path");
        assert!(l.locate(0x7f0000200100).is_none(), "deleted file");
        assert!(l.locate(0x7f0000300100).is_none(), "memfd");
        assert!(l.locate(0x1).is_none());
    }

    #[test]
    fn paths_with_spaces_and_garbage_lines() {
        let text = "1000-2000 r-xp 00000000 fd:01 1 /opt/my app/bin\nnot a maps line\n\n";
        let l = parse(text, |_| None);
        assert_eq!(l.modules[0].path, "/opt/my app/bin");
        assert!(l.locate(0x1500).is_some());
    }

    #[test]
    fn the_live_layout_contains_this_binary() {
        let l = Layout::current();
        let here = l.locate(the_live_layout_contains_this_binary as *const () as usize as u64);
        let (m, _) = here.expect("our own code is in a module");
        assert!(m.path.contains(
            &std::env::current_exe()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        ));
    }
}
