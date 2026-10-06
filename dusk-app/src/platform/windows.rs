//! The Windows file dialogs, `IFileOpenDialog` and `IFileSaveDialog`, and keys' virtual-key
//! codes.

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, MAPVK_VSC_TO_VK_EX, MapVirtualKeyExW,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FILEOPENDIALOGOPTIONS, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FOS_STRICTFILETYPES, FileOpenDialog, FileSaveDialog,
    IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{HSTRING, PCWSTR, w};

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

const VIDEOS: &[COMDLG_FILTERSPEC] = &[
    COMDLG_FILTERSPEC {
        pszName: w!("Videos"),
        pszSpec: w!(
            "*.mp4;*.m4v;*.mov;*.mkv;*.webm;*.avi;*.mts;*.m2ts;*.ts;*.3gp;*.wmv;*.flv;*.mpg;*.mpeg"
        ),
    },
    COMDLG_FILTERSPEC {
        pszName: w!("All files"),
        pszSpec: w!("*.*"),
    },
];

const PROGRAMS: &[COMDLG_FILTERSPEC] = &[COMDLG_FILTERSPEC {
    pszName: w!("Programs"),
    pszSpec: w!("*.exe"),
}];

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
            Dialog::ImportMedia
            | Dialog::OpenProject
            | Dialog::OpenVideo
            | Dialog::OpenProgram
            | Dialog::FindMedia { .. } => {
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
                match dialog {
                    Dialog::ImportMedia => {
                        picker.SetFileTypes(MEDIA)?;
                        picker.SetTitle(w!("Import media"))?;
                    }
                    Dialog::OpenVideo => {
                        picker.SetFileTypes(VIDEOS)?;
                        picker.SetTitle(w!("Compress a video"))?;
                    }
                    Dialog::OpenProgram => {
                        picker.SetFileTypes(PROGRAMS)?;
                        picker.SetTitle(w!("Your own ffmpeg.exe"))?;
                    }
                    Dialog::FindMedia { name, folder } => {
                        // The file's own name first, so its folder shows it alone; the
                        // strings live until the dialog has taken them.
                        let name = HSTRING::from(name.as_str());
                        let title = HSTRING::from(format!("Find {name}"));
                        picker.SetFileTypes(&[
                            COMDLG_FILTERSPEC {
                                pszName: PCWSTR(name.as_ptr()),
                                pszSpec: PCWSTR(name.as_ptr()),
                            },
                            MEDIA[0],
                            MEDIA[1],
                        ])?;
                        picker.SetFileName(&name)?;
                        picker.SetTitle(&title)?;
                        // Its old folder, when it is still there; otherwise the dialog's own.
                        if let Some(folder) = folder
                            && let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                                &HSTRING::from(folder.as_path()),
                                None::<&IBindCtx>,
                            )
                        {
                            picker.SetFolder(&folder)?;
                        }
                    }
                    _ => {
                        picker.SetFileTypes(PROJECTS)?;
                        picker.SetTitle(w!("Open project"))?;
                    }
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
            Dialog::SaveProject { .. } | Dialog::Export { .. } => {
                let picker: IFileSaveDialog =
                    CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
                // A file gets the format's extension whatever is typed, so what is written
                // matches its name.
                picker.SetOptions(FILEOPENDIALOGOPTIONS(
                    picker.GetOptions()?.0 | options(&[FOS_OVERWRITEPROMPT, FOS_STRICTFILETYPES]).0,
                ))?;
                match dialog {
                    Dialog::Export { suggested, kind } => {
                        // The filter's strings live until the dialog has taken them.
                        let extension = HSTRING::from(
                            suggested
                                .extension()
                                .map(|ext| ext.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                        let kind = HSTRING::from(kind.as_str());
                        let spec = HSTRING::from(format!("*.{extension}"));
                        picker.SetFileTypes(&[COMDLG_FILTERSPEC {
                            pszName: PCWSTR(kind.as_ptr()),
                            pszSpec: PCWSTR(spec.as_ptr()),
                        }])?;
                        picker.SetDefaultExtension(&extension)?;
                        if let Some(name) = suggested.file_name() {
                            picker.SetFileName(&HSTRING::from(name))?;
                        }
                        // The folder it suggests; a folder that is gone leaves the choice to
                        // the dialog.
                        if let Some(folder) = suggested.parent()
                            && let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                                &HSTRING::from(folder),
                                None::<&IBindCtx>,
                            )
                        {
                            picker.SetFolder(&folder)?;
                        }
                        picker.SetTitle(w!("Export"))?;
                    }
                    _ => {
                        let Dialog::SaveProject { suggested } = dialog else {
                            return Ok(Vec::new());
                        };
                        picker.SetFileTypes(PROJECTS)?;
                        picker.SetDefaultExtension(w!("dusk"))?;
                        picker.SetFileName(&HSTRING::from(suggested.as_str()))?;
                        picker.SetTitle(w!("Save project"))?;
                    }
                }
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

/// The character of the key at `scancode` in the calling thread's keyboard layout, from its
/// virtual-key code (see `character_of`); `None` for every other key.
pub fn key_character(scancode: u32) -> Option<char> {
    // SAFETY: both calls only read the keyboard layout of the calling thread, the UI thread,
    // whose layout is the one the user picked for Dusk's window.
    let code =
        unsafe { MapVirtualKeyExW(scancode, MAPVK_VSC_TO_VK_EX, Some(GetKeyboardLayout(0))) };
    character_of(code)
}

/// The character of virtual-key code `code` if it is a letter or a digit, lowercase, or a
/// punctuation key, as a US layout types it without Shift.
fn character_of(code: u32) -> Option<char> {
    match code {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(code).map(|c| c.to_ascii_lowercase()),
        0xBA => Some(';'),
        0xBB => Some('='),
        0xBC => Some(','),
        0xBD => Some('-'),
        0xBE => Some('.'),
        0xBF => Some('/'),
        0xC0 => Some('`'),
        0xDB => Some('['),
        0xDC => Some('\\'),
        0xDD => Some(']'),
        0xDE => Some('\''),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_punctuation_keys_have_a_character() {
        assert_eq!(character_of(0x4C), Some('l'));
        assert_eq!(character_of(0x31), Some('1'));
        // Punctuation keys, as a US layout types them without Shift.
        assert_eq!(character_of(0xBF), Some('/'));
        assert_eq!(character_of(0xBB), Some('='));
        assert_eq!(character_of(0xDC), Some('\\'));
        assert_eq!(character_of(0xDE), Some('\''));
        // The space bar, the numeric keypad's 1 and F1.
        for code in [0x20, 0x61, 0x70] {
            assert_eq!(character_of(code), None);
        }
    }
}
