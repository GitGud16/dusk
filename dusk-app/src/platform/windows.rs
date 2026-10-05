//! The Windows file dialogs: `IFileOpenDialog` and `IFileSaveDialog`.

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FILEOPENDIALOGOPTIONS, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FileOpenDialog, FileSaveDialog, IFileOpenDialog,
    IFileSaveDialog, IShellItem, SIGDN_FILESYSPATH,
};
use windows::core::{HSTRING, w};

use super::Dialog;

/// Files Dusk imports, by extension. Anything else FFmpeg reads can still be picked with
/// "All files".
const MEDIA: &[COMDLG_FILTERSPEC] = &[
    COMDLG_FILTERSPEC {
        pszName: w!("Video, audio and images"),
        pszSpec: w!(
            "*.mp4;*.m4v;*.mov;*.mkv;*.webm;*.avi;*.mts;*.m2ts;*.ts;*.3gp;*.wmv;*.flv;*.mpg;*.mpeg;\
             *.mp3;*.wav;*.flac;*.aac;*.m4a;*.ogg;*.oga;*.opus;*.amr;*.wma;*.aif;*.aiff;\
             *.jpg;*.jpeg;*.png;*.webp;*.tif;*.tiff;*.heic;*.heif;*.bmp;*.gif"
        ),
    },
    COMDLG_FILTERSPEC {
        pszName: w!("All files"),
        pszSpec: w!("*.*"),
    },
];

const PROJECTS: &[COMDLG_FILTERSPEC] = &[COMDLG_FILTERSPEC {
    pszName: w!("Dusk projects"),
    pszSpec: w!("*.dusk"),
}];

/// Shows `dialog` owned by the window `owner` and waits for it; what was picked, or nothing.
pub fn show(owner: Option<isize>, dialog: &Dialog) -> Vec<PathBuf> {
    // SAFETY: COM is set up for this thread alone, every COM object `pick` makes is released
    // before it returns, and COM is torn down only if this call set it up.
    unsafe {
        let set_up =
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let owner = owner.map(|handle| HWND(handle as *mut c_void));
        let picked = pick(owner, dialog).unwrap_or_default();
        if set_up {
            CoUninitialize();
        }
        picked
    }
}

/// # Safety
///
/// COM must be set up on the calling thread.
unsafe fn pick(owner: Option<HWND>, dialog: &Dialog) -> windows::core::Result<Vec<PathBuf>> {
    let options = |extra: &[FILEOPENDIALOGOPTIONS]| {
        let all = [FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST]
            .iter()
            .chain(extra)
            .fold(0, |all, option| all | option.0);
        FILEOPENDIALOGOPTIONS(all)
    };
    // SAFETY: plain COM calls on objects made here; COM is set up (the caller's promise).
    unsafe {
        match dialog {
            Dialog::ImportMedia | Dialog::OpenProject => {
                let picker: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
                let import = matches!(dialog, Dialog::ImportMedia);
                let multiple: &[FILEOPENDIALOGOPTIONS] = if import {
                    &[FOS_FILEMUSTEXIST, FOS_ALLOWMULTISELECT]
                } else {
                    &[FOS_FILEMUSTEXIST]
                };
                picker.SetOptions(FILEOPENDIALOGOPTIONS(
                    picker.GetOptions()?.0 | options(multiple).0,
                ))?;
                if import {
                    picker.SetFileTypes(MEDIA)?;
                    picker.SetTitle(w!("Import media"))?;
                } else {
                    picker.SetFileTypes(PROJECTS)?;
                    picker.SetTitle(w!("Open project"))?;
                }
                // Cancelling is an error too; either way nothing was picked.
                if picker.Show(owner).is_err() {
                    return Ok(Vec::new());
                }
                let items = picker.GetResults()?;
                (0..items.GetCount()?)
                    .map(|index| path_of(&items.GetItemAt(index)?))
                    .collect()
            }
            Dialog::SaveProject { suggested } => {
                let picker: IFileSaveDialog =
                    CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
                picker.SetOptions(FILEOPENDIALOGOPTIONS(
                    picker.GetOptions()?.0 | options(&[FOS_OVERWRITEPROMPT]).0,
                ))?;
                picker.SetFileTypes(PROJECTS)?;
                picker.SetDefaultExtension(w!("dusk"))?;
                picker.SetFileName(&HSTRING::from(suggested.as_str()))?;
                picker.SetTitle(w!("Save project"))?;
                if picker.Show(owner).is_err() {
                    return Ok(Vec::new());
                }
                Ok(vec![path_of(&picker.GetResult()?)?])
            }
        }
    }
}

/// # Safety
///
/// COM must be set up on the calling thread.
unsafe fn path_of(item: &IShellItem) -> windows::core::Result<PathBuf> {
    // SAFETY: the name is a string the shell allocated for us; it is copied, then freed once.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = PathBuf::from(OsString::from_wide(name.as_wide()));
        CoTaskMemFree(Some(name.0 as *const c_void));
        Ok(path)
    }
}
