//! Where loa keeps its files (spec 8-2).

use crate::error::{LoaError, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// `~/Library/Application Support/loa` on macOS, `%APPDATA%\loa` on Windows.
    pub fn discover() -> Result<Paths> {
        #[cfg(feature = "test-hooks")]
        if let Ok(home) = std::env::var("LOA_HOME") {
            return Ok(Paths {
                root: PathBuf::from(home),
            });
        }
        let base = dirs::config_dir()
            .ok_or_else(|| LoaError::other("設定ディレクトリの場所を特定できません"))?;
        Ok(Paths {
            root: base.join("loa"),
        })
    }

    pub fn at(root: impl Into<PathBuf>) -> Paths {
        Paths { root: root.into() }
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn scenarios_dir(&self) -> PathBuf {
        self.root.join("scenarios")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }
    pub fn tasks_dir(&self) -> PathBuf {
        self.state_dir().join("tasks")
    }
    pub fn task_dir(&self, id: &str) -> PathBuf {
        self.tasks_dir().join(id)
    }
    pub fn locks_dir(&self) -> PathBuf {
        self.state_dir().join("locks")
    }
    pub fn resources_file(&self) -> PathBuf {
        self.state_dir().join("resources.json")
    }
    pub fn terms_file(&self) -> PathBuf {
        self.state_dir().join("terms.json")
    }
    pub fn reports_dir(&self) -> PathBuf {
        self.root.join("reports")
    }
    /// Working directory and config used for every agent-browser invocation.
    pub fn browser_dir(&self) -> PathBuf {
        self.root.join("browser")
    }
    pub fn browser_config(&self) -> PathBuf {
        self.browser_dir().join("agent-browser.json")
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [
            self.root.clone(),
            self.scenarios_dir(),
            self.state_dir(),
            self.tasks_dir(),
            self.locks_dir(),
            self.reports_dir(),
            self.browser_dir(),
        ] {
            create_private_dir(&d)?;
        }
        Ok(())
    }
}

/// Create a directory readable only by the current user.
pub fn create_private_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p)
        .map_err(|e| LoaError::other(format!("{} を作成できません: {e}", p.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Atomically replace a file (write to a temp file then rename). Mode 0600 on Unix.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| LoaError::other("書き込み先が不正です"))?;
    std::fs::create_dir_all(dir)?;
    let mut rnd = [0u8; 6];
    let _ = getrandom::fill(&mut rnd);
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("f"),
        hex::encode(rnd)
    ));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        LoaError::other(format!("{} を書き込めません: {e}", path.display()))
    })
}
