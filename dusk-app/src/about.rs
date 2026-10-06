//! The About dialog (docs/ARCHITECTURE.md, "Release (0.1)"): Dusk's version and license,
//! what it is built with, the third-party licenses installed beside it and the releases
//! page. The system opens both, so Dusk itself makes no request.

use std::path::{Path, PathBuf};

use crate::app::App;

/// Where new versions of Dusk are published: the repository's releases page.
pub const RELEASES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");

/// The folder of license files the installer puts beside `exe`.
pub fn licenses_dir(exe: &Path) -> Option<PathBuf> {
    exe.parent().map(|dir| dir.join("licenses"))
}

/// The folder of license files beside the running Dusk, when there is one: a development
/// build has none.
fn installed_licenses() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    licenses_dir(&exe).filter(|dir| dir.is_dir())
}

impl App {
    /// Help → About Dusk and Shift+F1: the About dialog opens, unless another dialog is.
    pub(crate) fn open_about(&mut self) {
        if self.main_dialog_open() {
            return;
        }
        let Some(window) = self.window() else {
            return;
        };
        window.set_about_licenses(installed_licenses().is_some());
        window.set_about_open(true);
        self.about_open = true;
    }

    pub fn about_close(&mut self) {
        self.about_open = false;
        if let Some(window) = self.window() {
            window.set_about_open(false);
        }
    }

    /// Licenses: the folder of license files opens in Explorer.
    pub fn about_licenses(&self) {
        let Some(dir) = installed_licenses() else {
            return self.fail("The license files are installed with Dusk, beside dusk.exe.");
        };
        if let Err(error) = crate::platform::show_folder(&dir) {
            self.fail(&format!(
                "Dusk could not open {}: {error}. Open that folder yourself to read the licenses.",
                dir.display()
            ));
        }
    }

    /// The system could not open the releases page.
    pub fn about_link_failed(&self) {
        self.fail(&format!(
            "Your browser did not open. The releases page is {RELEASES}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_releases_page_is_the_repositorys() {
        assert_eq!(RELEASES, "https://github.com/GitGud16/dusk/releases");
    }

    #[test]
    fn the_licenses_are_in_a_folder_beside_dusk() {
        let exe = Path::new(r"C:\Users\someone\AppData\Local\Programs\Dusk\dusk.exe");
        assert_eq!(
            licenses_dir(exe),
            Some(PathBuf::from(
                r"C:\Users\someone\AppData\Local\Programs\Dusk\licenses"
            ))
        );
        assert_eq!(
            licenses_dir(Path::new("dusk.exe")),
            Some(PathBuf::from("licenses"))
        );
    }
}
