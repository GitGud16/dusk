# Code signing policy

Dusk's releases are to be signed so that Windows names their publisher (ROADMAP.md, M7). **Status: not signed yet.** Dusk is to be signed through SignPath Foundation, which signs open-source projects for free; until it accepts Dusk, the installer and the programs it installs are unsigned, and the README says how to get past Windows' warning.

Once accepted: free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

## What is signed

- `dusk.exe` and `dusq.exe`, and the installer, `dusk-<version>-setup.exe`.
- Only what is built from this repository's source, by its release workflow (`.github/workflows/release.yml`), from a pushed `v*` tag: SignPath checks that each file it signs came from that build.
- Not FFmpeg's DLLs, which Dusk ships beside the programs unchanged. They are built from FFmpeg's source, not Dusk's, and SignPath signs a project's own binaries only. Not Inno Setup's uninstaller, `unins000.exe`, which the installer writes as it installs.

## Team roles

- **Committers and reviewers:** the repository's [collaborators](https://github.com/GitGud16/dusk/graphs/contributors) with write access, who review every change from anyone else before it is merged.
- **Approvers:** the repository's owner, [GitGud16](https://github.com/GitGud16), who approves each release's signing request in SignPath by hand.

Every member uses multi-factor authentication for GitHub and for SignPath.

## Privacy

This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it. Dusk has no network code at all (REQUIREMENTS.md, "Offline and private"); the one link it has, to the releases page in its About dialog, is opened by the system's browser when the user clicks it.
