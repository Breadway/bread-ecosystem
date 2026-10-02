use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
    prefix: PathBuf,
    state_home: PathBuf,
    config_home: PathBuf,
    data_home: PathBuf,
    stub_bin: PathBuf,
    systemctl_log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let prefix = home.join(".local");
        let state_home = temp.path().join("state");
        let config_home = temp.path().join("config");
        let data_home = temp.path().join("data");
        let stub_bin = temp.path().join("bin");
        let systemctl_log = temp.path().join("systemctl.log");

        for path in [
            &home,
            &prefix,
            &state_home,
            &config_home,
            &data_home,
            &stub_bin,
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let stub = stub_bin.join("systemctl");
        fs::write(
            &stub,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$BAKERY_SYSTEMCTL_LOG\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        }

        Self {
            _temp: temp,
            home,
            prefix,
            state_home,
            config_home,
            data_home,
            stub_bin,
            systemctl_log,
        }
    }

    fn installed_path(&self) -> PathBuf {
        self.state_home.join("bakery/installed.json")
    }

    fn bin_path(&self) -> PathBuf {
        self.prefix.join("bin/fakepkg")
    }

    fn unit_path(&self) -> PathBuf {
        self.config_home.join("systemd/user/fakepkg.service")
    }

    fn seed_package(&self) {
        fs::create_dir_all(self.bin_path().parent().unwrap()).unwrap();
        fs::write(self.bin_path(), b"fake executable").unwrap();
        fs::create_dir_all(self.unit_path().parent().unwrap()).unwrap();
        fs::write(self.unit_path(), b"[Service]\nExecStart=/fakepkg\n").unwrap();
        fs::create_dir_all(self.prefix.join("share/licenses/fakepkg")).unwrap();
        fs::write(
            self.prefix.join("share/licenses/fakepkg/LICENSE"),
            b"license",
        )
        .unwrap();
        fs::create_dir_all(self.prefix.join("share/applications")).unwrap();
        fs::write(
            self.prefix.join("share/applications/fakepkg.desktop"),
            b"[Desktop Entry]\n",
        )
        .unwrap();
        fs::create_dir_all(self.prefix.join("share/fakepkg")).unwrap();
        fs::write(self.prefix.join("share/fakepkg/data"), b"user data").unwrap();
        fs::create_dir_all(self.config_home.join("fakepkg")).unwrap();
        fs::write(
            self.config_home.join("fakepkg/settings.toml"),
            b"setting = true\n",
        )
        .unwrap();
        let state = r#"{
  "track": "stable",
  "packages": {
    "fakepkg": {
      "name": "fakepkg",
      "version": "1.0.0",
      "binaries": ["fakepkg"],
      "services": ["fakepkg.service"],
      "installed_at": "2026-10-01T00:00:00Z",
      "track": "stable",
      "previous_version": null,
      "binary_sha256": {}
    }
  }
}"#;
        fs::create_dir_all(self.installed_path().parent().unwrap()).unwrap();
        fs::write(self.installed_path(), state).unwrap();
    }

    fn command(&self, args: &[&str]) -> Output {
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(self.stub_bin.clone()).chain(std::env::split_paths(&original_path)),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_bakery"))
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("BAKERY_PREFIX", &self.prefix)
            .env("BAKERY_SYSTEMCTL_LOG", &self.systemctl_log)
            .env("PATH", path)
            .args(args)
            .output()
            .unwrap()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_under(path: &Path, root: &Path) {
    assert!(path.starts_with(root));
}

#[test]
fn remove_dry_run_preserves_files_units_hooks_and_installed_state() {
    let fixture = Fixture::new();
    fixture.seed_package();
    let state_before = fs::read(fixture.installed_path()).unwrap();
    let purge_paths = [
        fixture.prefix.join("share/licenses/fakepkg/LICENSE"),
        fixture.prefix.join("share/applications/fakepkg.desktop"),
        fixture.prefix.join("share/fakepkg/data"),
        fixture.config_home.join("fakepkg/settings.toml"),
    ];

    let output = fixture.command(&["--dry-run", "--yes", "remove", "--purge", "fakepkg"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan = stdout(&output);
    assert!(plan.contains("would remove"), "{plan}");
    assert!(plan.contains("installed.json entry for fakepkg"), "{plan}");
    assert_under(&fixture.bin_path(), &fixture.prefix);
    assert_under(&fixture.unit_path(), &fixture.config_home);
    assert!(fixture.bin_path().exists());
    assert!(fixture.unit_path().exists());
    assert!(purge_paths.iter().all(|path| path.exists()));
    assert_eq!(fs::read(fixture.installed_path()).unwrap(), state_before);
    assert!(!fixture.systemctl_log.exists());
}

#[test]
fn remove_no_hooks_skips_systemctl_side_effects() {
    let fixture = Fixture::new();
    fixture.seed_package();

    let output = fixture.command(&["--no-hooks", "--yes", "remove", "fakepkg"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.bin_path().exists());
    assert!(!fixture.unit_path().exists());
    assert!(!fixture.systemctl_log.exists());
    let state = fs::read_to_string(fixture.installed_path()).unwrap();
    assert!(!state.contains("fakepkg"));
}
