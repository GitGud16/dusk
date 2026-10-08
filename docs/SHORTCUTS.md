# Keyboard shortcuts

Every action in Dusk has keys; these are the ones it starts with. In Dusk, `?` or F1 opens this list, to search it and to change keys. Changed keys are kept in `shortcuts.txt` in Dusk's settings folder (`%APPDATA%\Dusk` on Windows), one action a line under the names below, such as `split = S`, or `redo = Ctrl+Shift+Z, Ctrl+Y` for two keys.

Letter and digit keys go by their place on the keyboard, so they work under any layout. This file is written from Dusk's own table of shortcuts, and a test keeps the two the same.

## Playback

| Keys | What it does | In `shortcuts.txt` |
|---|---|---|
| `Space` | Play or pause | `play-pause` |
| `J` | Play backwards; press again to play faster, up to 32x | `play-backward` |
| `K` | Pause | `pause` |
| `L` | Play; press again to play faster, up to 32x | `play-forward` |
| `Shift+J` | Play backwards slowly; press again to play slower, down to 0.1x | `play-slow-backward` |
| `Shift+L` | Play slowly; press again to play slower, down to 0.1x | `play-slow-forward` |
| `Left` | Previous frame | `previous-frame` |
| `Right` | Next frame | `next-frame` |
| `Home` | Go to the start | `go-to-start` |
| `End` | Go to the last frame | `go-to-end` |
| `Up` | Go to the previous cut | `previous-cut` |
| `Down` | Go to the next cut, where any clip starts or ends | `next-cut` |
| `Shift+Left` | Back one second | `back-a-second` |
| `Shift+Right` | Ahead one second | `ahead-a-second` |

## Editing

| Keys | What it does | In `shortcuts.txt` |
|---|---|---|
| `Ctrl+Z` | Undo | `undo` |
| `Ctrl+Shift+Z`, `Ctrl+Y` | Redo | `redo` |
| `S` | Split at the playhead: the selected clip, or every clip there | `split` |
| `Delete` | Delete the selected clip and its linked clips, leaving a gap | `delete` |
| `Shift+Delete` | Delete and close the gap on every unlocked track | `ripple-delete` |
| `Alt+Delete` | Delete the selected clip only, not its linked clips | `delete-one` |
| `Ctrl+L` | Detach audio: unlink the selected clip | `unlink` |
| `E` | Enable or disable the selected clip | `toggle-enabled` |
| `F` | Fit the picture with bars, or fill the frame and crop | `toggle-fill` |
| `P` | Place the selected media at the playhead, and go past it | `place` |
| `Alt+Up` | Select the media above in the bin | `previous-media` |
| `Alt+Down` | Select the media below in the bin | `next-media` |
| `D` | Select the clip at the playhead; press again for the one below it | `select-at-playhead` |
| `Ctrl+Shift+A` | Select no clip | `select-none` |
| `I` | Start the clip at the playhead: the selected one, or the clip editor's | `mark-in` |
| `O` | End the clip at the playhead: the selected one, or the clip editor's | `mark-out` |
| `Enter` | Open the selected clip in the clip editor | `open-clip-editor` |

## Clip editor

| Keys | What it does | In `shortcuts.txt` |
|---|---|---|
| `R` | Clip editor: turn the picture right | `turn-right` |
| `Shift+R` | Clip editor: turn the picture left | `turn-left` |
| `H` | Clip editor: mirror the picture left to right | `mirror-left-right` |
| `V` | Clip editor: mirror the picture top to bottom | `mirror-top-bottom` |
| `Ctrl+Enter` | Clip editor: apply the changes to the project | `apply-clip` |
| `Ctrl+W` | Close the clip editor | `close-clip-editor` |
| `Ctrl+R` | Clip editor: the clip changed in the main window; take it as it is now | `reload-clip` |
| `Ctrl+K` | Clip editor: the clip changed in the main window; keep the draft | `keep-draft` |
| `Ctrl+Shift+E` | Clip editor: export the clip as a file of its own | `export-clip` |

## Tracks

| Keys | What it does | In `shortcuts.txt` |
|---|---|---|
| `Alt+1` | Hide or show V1 | `mute-v1` |
| `Alt+2` | Hide or show V2 | `mute-v2` |
| `Alt+3` | Mute or unmute A1 | `mute-a1` |
| `Alt+4` | Mute or unmute A2 | `mute-a2` |
| `Ctrl+Alt+1` | Lock or unlock V1 | `lock-v1` |
| `Ctrl+Alt+2` | Lock or unlock V2 | `lock-v2` |
| `Ctrl+Alt+3` | Lock or unlock A1 | `lock-a1` |
| `Ctrl+Alt+4` | Lock or unlock A2 | `lock-a2` |

## View

| Keys | What it does | In `shortcuts.txt` |
|---|---|---|
| `=` | Zoom in | `zoom-in` |
| `-` | Zoom out | `zoom-out` |
| `\` | Fit the sequence in view | `zoom-fit` |
| `?`, `F1` | Show the keyboard shortcuts | `shortcut-list` |
| `Shift+F1` | About Dusk: its version, its license and what it is built with | `about` |

## Project

| Keys | What it does | In `shortcuts.txt` |
|---|---|---|
| `Ctrl+N` | New project | `new-project` |
| `Ctrl+O` | Open a project | `open-project` |
| `Ctrl+S` | Save the project | `save` |
| `Ctrl+Shift+S` | Save the project under a new name | `save-as` |
| `Ctrl+I` | Import media | `import` |
| `Ctrl+Shift+R` | Sequence settings: frame rate and size | `sequence-settings` |
| `Ctrl+Comma` | Settings: the frame cache, the export default, your own ffmpeg.exe | `settings` |
| `Ctrl+E` | Export the timeline | `export` |
| `Esc` | Cancel the export | `cancel-export` |
| `Ctrl+M` | Compress a video into a smaller file | `compress-video` |
| `Ctrl+Shift+M` | Find the media files the project cannot find | `find-missing-media` |
| `Ctrl+Q` | Quit | `quit` |
