# Dusk — Behavior

What Dusk does in front of the user: the clip editor's window, the export and compress dialogs, the keyboard and settings, finding missing media, and the messages. Each section continues the [ARCHITECTURE.md](ARCHITECTURE.md) section of the same name, which holds the design underneath (the model, the engine, the export pipeline). Like ARCHITECTURE.md, this file says what Dusk does now; why it is that way is in [DECISIONS.md](DECISIONS.md) under the same headings. Colors and type are in [THEME.md](THEME.md), every action's keys in [SHORTCUTS.md](SHORTCUTS.md), and what ships in [RELEASE.md](RELEASE.md).

## Pop-out clip editor

### The window

A double-click on a timeline clip opens it, as do Enter on the selected clip, the Edit menu and the properties panel. There is one clip editor at a time. Opening a clip of another group while the draft has changes not applied asks *Apply*, *Discard* or *Cancel* first, and so do closing the window and leaving the project (New, Open, Quit), the latter before the question about unsaved changes.

- Trimming is a bar over the whole source with in and out handles, and I and O set them at the playhead; trim points snap to the source's own frames (the sequence's for sound).
- The crop is shown and typed as the pixels cut from each side of the picture as it shows, after the turn and the flips, and kept as the model's rectangle of the upright picture; a quarter turn of a mirrored picture swaps the two flips, so the mirror stays the same way on screen.
- A trim or a speed change that shortens the clip shortens its fades, fade-in first, as a trim on the timeline does.
- Every change shows in the preview at once, while it plays too; sound edits are heard from the next play, since the mixer takes the project when playback starts, as in the main window.
- A change that a rule of the timeline refuses (a crop larger than the picture, fades longer than the clip) is not taken, and the window says why.

## Export details

### The export dialog

Export (Ctrl+E, the toolbar, the File menu) opens it over the main window for the timeline, and *Export as file* over the clip editor for its clip. It offers *Video* or *Sound only* (each only when there is a picture or sound), then the file format, the codec with the encoder that will write it (on the graphics card, or in software), the picture size (the sequence's own, then each preset below its short side, so nothing is enlarged), the Quality presets and slider, the sound codec when the format holds two, and an Advanced section; for sound alone, the sound file format. The first time it opens in a session it shows that it is checking the encoders while the encoder probe runs (ARCHITECTURE.md, "Encoder selection order"), and Export waits for it. Export… asks where with the system's save dialog, which gives the file the format's extension whatever is typed, so what is written matches its name. The dialog starts from the last export's choices in the session, and keeps its top edge while rows appear and go, so a choice never moves the rows above it.

### The compress dialog

File → *Compress a video…* (Ctrl+M) asks for a video with the system's open dialog and reads it on a worker while one dialog opens, which shows its length, picture size and file size and aims it either at a file size, in whole megabytes with 8, 25, 50 and 100 MB at hand (25 MB to start with, or half a video already smaller than that), or at the Quality slider at the video's own size. For a size it says what the file will come out as (the picture size the ladder gives, the video and sound bitrates), and below the floor it names the smallest size, rounded up to a whole megabyte, with a button to take it, and Compress waits. The file is saved with the system's save dialog as "<name> compressed.mp4" beside the video by default. Progress and cancelling are the export's, a second pass is announced, and the end says the file's actual size.

## Keyboard and settings

Dusk can be used from the keyboard alone, except to drag clips. `dusk-app/tests/keyboard.rs` holds it to that: it starts the real binary on a saved project, edits it with keys posted to the window and saves it through the question that closing asks.

### Shortcuts

One table in `dusk-app` (`shortcuts::ACTIONS`, one entry an action with its group) holds every action with its default keys and what it does; menu items and buttons name the same actions, and a test keeps every action they name in the table. The keys in use are `keymap::Keymap`, which both windows' key handling, menus and shortcut list go through. Every action has a stable name (`play-pause`, `split`, `mute-v1`), which the shortcuts file and the shortcut list use.

- Keys are written as modifiers and a key joined by `+`, such as `Ctrl+Shift+Z`, where the key is a letter, a digit, a punctuation character or a named key (`Space`, `Enter`, `Esc`, `Tab`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Left`, `Right`, `Up`, `Down`, `F1` to `F12`, and `Comma` and `Plus`, since those two characters set keys and modifiers apart in the file).
- Shift with a punctuation key stands for the character it types on a US layout, as a press of it arrives (`Shift+/` is `?`).
- Letter and digit keys match by the key's place on the keyboard (its virtual-key code), so they work under any layout; punctuation matches by the character typed, with or without Shift, and where the layout types something else on that key (Arabic types ؟ where a US layout types ?), by what the key types on a US layout.

### Remapping

`shortcuts.txt` in Dusk's settings folder holds the keys the user changed, one action a line (`split = S`, or `redo = Ctrl+Shift+Z, Ctrl+Y` for two keys); every other action is written there as a comment with its default keys, so the file names every action, and a later version's new defaults still reach a user who never changed them. The first time Dusk starts it writes the file with every action as a comment, ready to change.

- A line Dusk cannot read (an action it does not know, a key it cannot read, a key an earlier line took) is skipped and reported when Dusk starts, and that action keeps its default. What Dusk cannot read is said in the status line and at the top of the shortcut list, which stays where a later message would cover the status line.
- The shortcut list changes keys too: pick an action and press its new keys, Tab among them; a key another action has is taken from it only when pressed a second time; *Remove* and *Default* do what they say. Dusk writes the file when keys change there (at once, on the file worker) and reads it once when it starts.

### The shortcut list

It opens with `?` and F1, a toolbar button and Help → Keyboard shortcuts, in either window. It groups the actions (Playback, Editing, Clip editor, Tracks, View, Project), narrows them to what one types (what they do, their keys, or their name in the file), and changes keys as above. In the main window it is a dialog (`shortcut_dialog`, with its logic tested apart from the window) whose search field has the keys when it opens; Up and Down pick an action, Enter or a double-click waits for its new keys (the keys leave the search field meanwhile, and Escape stops waiting), and the list keeps the picked action in view. The clip editor's `?` shows the list without changing it.

### Without the mouse

Tab and Shift+Tab move through every control of a panel or dialog, which shows a focus ring in `primary-bright`; the arrow keys change a segmented choice or a slider, Space or Enter presses a button, and Escape gives the keys back to the shortcuts. Keys the focused control does not use go on to the shortcuts, so Space still plays while a button has focus.

- Every control in `widgets.slint` has a focus scope that Tab reaches and a click does not take, so the mouse leaves the keys with the shortcuts. Number fields select their number when reached and count with Up and Down. The frame rates in Sequence settings are one control for Tab, chosen with the arrow keys (Left and Right along a row, Up and Down between rows).
- A click on a button leaves the keyboard in a number field, so the number fields of the export, compress and sequence dialogs, whose numbers are only choices until a button takes them, report each number as it is typed. The Settings dialog applies a typed cache size as it closes. The clip editor's and the properties panel's fields apply on Enter or when the keyboard leaves them, since the preview shows whether a number took and a speed taken as it is typed would cut the clip at the gap on the way.
- Slint walks the whole window with Tab, so while a dialog is open a per-window `Modal` global keeps the controls behind it out of reach, which keeps Tab in the dialog and Enter off any button behind it, and the keys go back to the window whenever a dialog opens or closes. A question (closing with unsaved changes, say) is drawn over every dialog and takes the keys first, and while it is open only its own buttons are reached with Tab (`Modal` has a question level). While a dialog is open the menu's other actions wait, and the project keys the clip editor passes on say so there, except Quit, which asks its question over the dialog.
- Keys for editing without the mouse (`navigation`): Up and Down go to the previous or next cut (any clip's start or end); Shift+Left and Shift+Right move a second; D selects the clip at the playhead, the topmost first and the next one down when pressed again; Ctrl+Shift+A selects nothing; Alt+Up and Alt+Down pick the previous or next media in the bin; I and O start and end the selected clip at the playhead, as they do in the clip editor; and placing media moves the playhead to the end of the clip it placed, so placing again puts the next clip after it.
- The playhead can stand just past the last frame, where the preview is black, so that placing appends and the last cut can be reached; End still goes to the last frame. Keys that move the playhead scroll a zoomed-in timeline to keep it in view, as playback does. The media bin scrolls to keep the selected media in view.
- Shift+Left and Shift+Right work in the clip editor too; the cut and selection keys mean nothing there.

### Settings

File → Settings… (Ctrl+,) holds:

- the frame cache's size (384 MB by default, applied at once: a smaller cap gives back what lies above it; the field counts in steps of 64 MB with Up and Down);
- the export dialog's default (file format, codec, picture size, quality), which it starts from in each session until an export there changes it;
- the user's own `ffmpeg.exe` (its path, whether to use it, and what it has, checked on a worker when Dusk starts and when it is picked), which the export dialog's Advanced section also picks; both places keep it.

`settings.txt` holds them as `name = value` lines, with comments saying what each takes; a value Dusk cannot read is reported and its default used. The dialog (`settings_dialog`, with how each control changes the settings tested apart from the window) writes the file on the file worker after every change, and the first start writes it with the defaults, ready to change.

- Changing the export default makes the next export start from it, not from the session's last export. The export dialog's choice between Dusk's encoder and the user's program lasts for the session, as its other choices do, while the Settings switch is what the next session starts with.
- A program picked in either place is kept once it lists its encoders as `ffmpeg` does (with or without x264 and x265), and one that does not leaves the program in use, if any, as it was; a check overtaken by a later pick or by *Forget* changes nothing. When the kept program cannot be run as Dusk starts (on a drive that is not there, say), the status line says so and the setting stays, so it works again once the drive is back.
- Each line of either file that Dusk cannot read is reported with what Dusk does instead (the default is used, or the line is skipped); the status line shows the first and counts the rest.
- The limit on decoding one still stays at half the default cache (192 MB) whatever size the cache is given, so which photos a project can use does not depend on the setting.

### Where

Dusk's settings folder is `%APPDATA%\Dusk` on Windows (the roaming profile, so settings follow the user, while autosaves stay in `%LOCALAPPDATA%\Dusk`), and `$XDG_CONFIG_HOME/dusk` (`~/.config/dusk`) elsewhere; `DUSK_SETTINGS_DIR` moves it, which the tests use. Both files are plain UTF-8 text, small enough to read before the window shows, once, when Dusk starts; Dusk writes them on the file worker as `name.part` renamed into place, as it saves projects.

## Missing media

A project whose media files cannot be found opens anyway, and a dialog then lists them, each with *Find…*. The media bin marks a missing file and says how to find it, with the key that does it; File → Find missing media… (Ctrl+Shift+M) opens the dialog again. Relinking as a command, checking which files are missing, and what the engine does with a file that goes away are in ARCHITECTURE.md, "Missing media".

- **What a found file must hold.** A file picked there is read on a worker and used only if it holds what the project's clips use: the same kind of media, sound where a clip plays some, every clip's source range inside it (a project file whose clips reach past their media's end is refused when read, so a shorter file would leave a project that cannot be opened again), and a picture that every crop of it fits, since crops are kept in its upright pixels.
- **The others beside it.** Dusk then looks in the picked file's folder, under their own names, for the other missing files that were in the same folder as the one found, so a folder that moved, or a drive with a new letter, is relinked in one step. A missing file from another folder is not looked for there, since a file of its name in another folder is another file (cameras name files the same on every card). The picked file, and the files beside it named as those other missing ones in any case (one back at its own path too, as when a drive comes back), are read on a thread of their own, which says how far it has got and stops when the list closes or another find starts; the picked file must fit, the others that fit come along in the same undoable step, and those that do not are counted in a warning.
- **One file, one media.** The picked file is not found again for another missing file of its name, a file the project already uses is not taken, and a name two missing files share finds neither. A find whose media the project no longer names as it did when the file was asked for (another project opened, an edit undone meanwhile) changes nothing.
- **The dialog** (`missing`, whose matching and relinking are tested apart from the window) lists them with the folder each was in; Up and Down or a click pick one, and Find… (or Enter, or a double-click) asks for it with the system's dialog, which shows that file's name alone in its old folder when the folder is still there. When a question or another dialog is up as the list is to open (recovering an autosave as a project opens, say), the list opens once none is left open; when that is a question, the answer opens it, before any key pressed after the answer is handled.

## Error messages

Every message a user can see says what happened and what they can do (CLAUDE.md, "Code style").

- A media file that goes away while Dusk runs (an unplugged drive) is named with how to find it, and the bin marks it; `EngineError::missing_file` tells the app which file.
- The engine's errors reach the status line as sentences, capitalized like the app's own.
- An export's encoder runs in Dusk's helper, `dusq`: what the helper says went wrong is the export's error, word for word, and a helper that cannot start, or that stops without saying why, is named with what to do (export again, or reinstall Dusk).
- Where a message is passed on in a place its advice cannot be followed, the advice is left out: a project file that breaks a timeline rule names the rule (`Rejection::what`) without saying to trim a clip in a project that does not open, a failed recovery says what is wrong with the autosave (`FileError::what`), and a file picked in Find that cannot be read asks only for the file the project used.
- `dusk-app/tests/messages.rs` reads every crate's sources for gaps of spaces in strings (a line's lost `\`), following strings across lines as Rust reads them.
