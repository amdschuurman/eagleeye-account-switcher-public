//! Filesystem roots and the concrete paths derived from them. Roots are held in a struct so unit tests
//! can point them at a tempdir and exercise everything that touches disk on Linux.

use std::path::{Path, PathBuf};

/// The three OS roots we build every path from. On Windows these come from the environment; tests fill
/// them with a tempdir.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `%APPDATA%` (roaming): Battle.net.config, our own store, TcNo's cache.
    pub app_data: PathBuf,
    /// `%LOCALAPPDATA%`: Battle.net logs and CachedData.db.
    pub local_app_data: PathBuf,
    /// Our own store: `%APPDATA%\EagleEye Account Switcher`.
    pub store_dir: PathBuf,
}

impl Paths {
    /// Build from explicit roots (store defaults to `<app_data>\EagleEye Account Switcher`).
    pub fn new(app_data: PathBuf, local_app_data: PathBuf) -> Self {
        let store_dir = app_data.join("EagleEye Account Switcher");
        Paths { app_data, local_app_data, store_dir }
    }

    /// Discover the roots from the environment. On Windows these are `%APPDATA%` / `%LOCALAPPDATA%`;
    /// a dev build elsewhere falls back to a temp location so the app still starts.
    pub fn discover() -> Result<Self, String> {
        if let (Some(app), Some(local)) = (std::env::var_os("APPDATA"), std::env::var_os("LOCALAPPDATA")) {
            return Ok(Paths::new(PathBuf::from(app), PathBuf::from(local)));
        }
        #[cfg(windows)]
        {
            Err("APPDATA / LOCALAPPDATA are not set".into())
        }
        #[cfg(not(windows))]
        {
            let base = std::env::temp_dir().join("eagleeye-account-switcher-dev");
            Ok(Paths::new(base.join("roaming"), base.join("local")))
        }
    }

    /// Battle.net's roaming config directory (`%APPDATA%\Battle.net`).
    pub fn battlenet_config_dir(&self) -> PathBuf {
        self.app_data.join("Battle.net")
    }

    /// The Battle.net.config file itself.
    pub fn battlenet_config(&self) -> PathBuf {
        self.battlenet_config_dir().join("Battle.net.config")
    }

    /// Battle.net's log directory (`%LOCALAPPDATA%\Battle.net\Logs`).
    pub fn battlenet_logs_dir(&self) -> PathBuf {
        self.local_app_data.join("Battle.net").join("Logs")
    }

    /// Our account list.
    pub fn accounts_json(&self) -> PathBuf {
        self.store_dir.join("accounts.json")
    }

    /// Our settings file.
    pub fn settings_json(&self) -> PathBuf {
        self.store_dir.join("settings.json")
    }

    /// The vault directory (one `<code>.bin` per account).
    pub fn vault_dir(&self) -> PathBuf {
        self.store_dir.join("vault")
    }

    /// One account's encrypted vault file.
    pub fn vault_file(&self, code: &str) -> PathBuf {
        self.vault_dir().join(format!("{code}.bin"))
    }

    /// Battle.net's local cache database (`%LOCALAPPDATA%\Battle.net\CachedData.db`), read only for the
    /// BattleTag of each account in `login_cache`.
    pub fn battlenet_cached_db(&self) -> PathBuf {
        self.local_app_data.join("Battle.net").join("CachedData.db")
    }

    /// Account metadata that the engine does not own (category per account).
    pub fn meta_json(&self) -> PathBuf {
        self.store_dir.join("meta.json")
    }

    /// The Overwatch rank cache plus the manual overrides.
    pub fn ranks_json(&self) -> PathBuf {
        self.store_dir.join("ranks.json")
    }

    /// The locally stored Blizzard rank and role icons.
    pub fn icons_dir(&self) -> PathBuf {
        self.store_dir.join("icons")
    }

    /// The locally stored Overwatch player icons (inside `icons`, so the same asset scope serves them).
    pub fn avatars_dir(&self) -> PathBuf {
        self.icons_dir().join("avatars")
    }

    /// TcNo's Battle.net login cache (`%APPDATA%\TcNo Account Switcher\LoginCache\BattleNet`).
    pub fn tcno_battlenet_dir(&self) -> PathBuf {
        self.app_data
            .join("TcNo Account Switcher")
            .join("LoginCache")
            .join("BattleNet")
    }
}

/// The default Battle.net install folder when the registry and settings give nothing.
pub fn default_battlenet_install() -> &'static str {
    r"C:\Program Files (x86)\Battle.net"
}

/// The launcher executable inside a Battle.net install folder.
pub fn battlenet_launcher_exe(install_dir: &str) -> PathBuf {
    Path::new(install_dir).join("Battle.net Launcher.exe")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_derive_under_the_roots() {
        let p = Paths::new(PathBuf::from("/a/app"), PathBuf::from("/a/local"));
        assert_eq!(p.store_dir, PathBuf::from("/a/app/EagleEye Account Switcher"));
        assert_eq!(p.battlenet_config(), PathBuf::from("/a/app/Battle.net/Battle.net.config"));
        assert_eq!(p.battlenet_logs_dir(), PathBuf::from("/a/local/Battle.net/Logs"));
        assert_eq!(p.vault_file("12ab34cd"), PathBuf::from("/a/app/EagleEye Account Switcher/vault/12ab34cd.bin"));
        assert_eq!(p.battlenet_cached_db(), PathBuf::from("/a/local/Battle.net/CachedData.db"));
        assert_eq!(p.ranks_json(), PathBuf::from("/a/app/EagleEye Account Switcher/ranks.json"));
        assert_eq!(p.meta_json(), PathBuf::from("/a/app/EagleEye Account Switcher/meta.json"));
        assert_eq!(p.icons_dir(), PathBuf::from("/a/app/EagleEye Account Switcher/icons"));
        assert_eq!(p.avatars_dir(), PathBuf::from("/a/app/EagleEye Account Switcher/icons/avatars"));
        assert_eq!(
            p.tcno_battlenet_dir(),
            PathBuf::from("/a/app/TcNo Account Switcher/LoginCache/BattleNet")
        );
    }
}
