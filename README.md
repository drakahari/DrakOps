# DrakOps

**Home SysOps** · prerelease

DrakOps keeps the computers you manage in one place and opens SSH connections to them. Your device list stays on this computer. This is a prerelease. In the app, choose **Settings → About** to see the version.

## Downloads and walkthrough

- [Windows x64: DrakOps 0.1.0 RC5](https://github.com/drakahari/DrakOps/releases/tag/v0.1.0-rc.5): unsigned portable ZIP with Direct3D 12.
- [Linux x86_64: DrakOps 0.1.0 RC5](https://github.com/drakahari/DrakOps/releases/tag/v0.1.0-rc.5): shared archive, requires glibc 2.39 or newer and OpenGL.
- [Watch the one-device walkthrough on YouTube](https://youtu.be/O0VCrI6YF0Q) · [Download MP4](https://github.com/drakahari/DrakOps/releases/download/v0.1.0-rc.1/DrakOps-0.1.0-rc.1-Walkthrough.mp4).
- [macOS Apple Silicon: DrakOps 0.1.0 RC5](https://github.com/drakahari/DrakOps/releases/tag/v0.1.0-rc.5): prerelease ZIP, ad-hoc signed and not Apple-notarized.

## Quick start: connect to one computer

You’ll need a computer you control that already runs SSH, its address, and the username you use **on that computer**. This example is fictional; use your own details.

1. Click **+ Add device**. Enter **Display name**: Media Server; **Short alias**: media-server; **Saved host**: media-server.local; **SSH username**: alex; **SSH port**: 22. Leave the other fields blank for now. Click **Save device**.
2. Select **Media Server** and check that the target shows the intended username, address, and port. If this is your first time connecting to that host from this computer, choose **Open in external terminal**. Compare the server fingerprint shown in the prompt with a trusted copy from the server’s console or its administrator. Accept it only if they match. If a previously trusted fingerprint changes, stop and check why.
3. Back in DrakOps, click **Connect**, then click inside the terminal at the bottom to type. **Keyboard active** tells you when typing goes to the remote computer. Choose **Disconnect** when finished.

### Optional: avoid typing the server password next time

If you can already log in the way you want, skip this. If your server asks for its account password each time and you want to use an existing SSH key instead:

1. On the saved device, click **Set up SSH**, then **Use my SSH key** if the suggested key is the one you want. Otherwise choose a different **.pub** file.
2. Review the destination and public-key fingerprint, confirm the required checkboxes, and click **Install key in trusted terminal** when available. Enter any requested password in that terminal, not in DrakOps. Windows guided setup requires direct, standard POSIX OpenSSH and a matching local key pair. On Linux and macOS, run the system installer only once for a key not already installed. If guided setup is unavailable or unsuitable, choose **Copy reviewed public key** or ask your administrator.
3. Read the result in the trusted terminal; closing it does not confirm success. To try a fresh connection, close the setup terminal and choose **Open new SSH tab** under **Finish setup**. This leaves your existing tabs alone.

The file ending in **.pub** is the public key you may add to the intended server account. Its matching **private key** stays on your computer; never copy it to the server or paste it into DrakOps.

## Other features

- Add or edit an alias, display name, saved host, LAN and Tailscale addresses, SSH username and port, tags, and notes. Search across saved information. Routes are explicit per connection; the "Saved host" fallback order is saved host, Tailscale, LAN.
- Remove a selected device from DrakOps with a named confirmation. Undo the most recent removal until you close DrakOps; SSH settings, keys, and remote computers are not removed.
- Find devices reads the local Tailscale client only when requested. It makes no network scan, does not inspect SSH, and requires an SSH account and port review before adding checked peers. Offline peers can be saved; Tailscale online status is not proof of SSH access.
- **Find on LAN** lists local adapters. Choose your Wi-Fi or Ethernet adapter, then click **Find advertised SSH services** to look for local DNS-SD SSH announcements for up to four seconds. This does not scan ports, verify reachability, or connect. Review each selected address and advertised port, then enter the SSH username yourself. No devices appear if they don't announce SSH; add those manually or use Tailscale.
- Import existing `hosts.csv` as data. Review first. Rows with existing aliases are skipped; no existing shortcuts or configuration are changed.
- Copy an SSH command or connection details. **Open in external terminal** launches system OpenSSH in a separate window on Windows, Linux (including Omarchy), and macOS. If a launcher is unavailable, paste the copied command into your terminal. The app does not read passwords or private keys. Set up SSH reviews exactly one public key and offers standard installer handoff where available.
- Choose from 28 DLMS Learning Lab themes. The selected theme and inventory stay in the existing HomeOps app-data directory. Renaming the app does not move the data or create a second inventory. Treat host addresses, usernames, and notes as private; no keys or passwords belong in the inventory.

## Build and test

Use current stable Rust and the native platform prerequisites for eframe/egui. Windows MSVC is supported by the dependency stack; the first FM-16 development build uses the official Windows GNU Rust toolchain, with its self-contained linker tools on PATH for the current shell only. On Linux, the default GUI has X11 and Wayland support and requires the native OpenGL/desktop runtime. On macOS, use the native Apple tools.

```text
cargo test --locked
cargo build --locked
cargo run --locked -- --demo
```

The Unix safe-storage test uses a disposable directory beneath the checkout. If the checkout is below a shared writable parent such as `/tmp`, set `DRAKOPS_TEST_PRIVATE_DIR` to an existing private, writable test directory.

The demo contains only synthetic devices and advertisements, does not touch your regular profile, and disables live discovery and SSH. Launch without `--demo` for a fresh or existing inventory.

The current candidate does not run service probes, sync between computers, or change SSH settings. Validate the Windows, macOS, and Linux build separately before calling all three supported.


## Built-in SSH terminal (test candidate)

Open a device, check its route, then click **Connect**. One terminal opens below the device book. Resize it by dragging its top edge, collapse to keep it running, or click **Disconnect** before connecting to another device. Click the terminal to type, or Tab to the terminal and press F2. **Keyboard active** confirms capture; press **Ctrl+Esc** or click outside to release it. **Send Ctrl+C** is also available. Drag across terminal output to select and copy visible text with Ctrl+Shift+C (Linux/Windows) or Cmd+C (macOS). Use the terminal’s **Paste** button to open a local field. Paste with Ctrl+V (Linux/Windows) or Cmd+V (macOS), review the destination and text, then choose **Send to terminal**. Click the terminal again after sending to resume typing. macOS can also use Cmd+V while typing in the terminal. Paste sends one line without pressing Enter. Trailing line endings are removed; multiline text, control characters, and pastes over 4 KiB are refused. The terminal does not persist sessions or their contents. Use the mouse wheel or trackpad over the terminal, or drag its right scrollbar, to review up to 3,000 lines per tab in memory. Use **Older** or **Newer** to move one page without a pointer, or choose **Jump to live** to return to new output. Use **Find** to search this tab’s retained output for text and move through matches with **Previous** / **Next**. Search stays local and is disabled while a full-screen program controls the display. Full-screen programs keep their own display and keyboard controls, including Page Up and Page Down.

Use **A−** and **A+** to resize terminal text only. Click the point size to reset it. This lasts until you close DrakOps. Resizing a running tab asks the remote program to redraw; older output does not reflow when the terminal width changes. For clean copy across visual line wraps, choose **Select text** first: copy joins soft wraps but keeps real line breaks.

For a host not yet in your OpenSSH known-hosts file, use **Open in external terminal** or **Copy command** and independently verify the host fingerprint there first. If a known host key changes, stop and verify the change with the host owner. The embedded client requires an existing trusted entry and does not automatically add or update host keys. SSH authentication is provided by OpenSSH, not stored by DrakOps. Embedded tabs keep your OpenSSH routing, identities and known-host settings, but do not inherit configured port or X11 forwarding. Use an external terminal if you intend to forward traffic. Run `cargo run --locked -- --demo --demo-terminal` for a no-connection preview.

Device type is optional. Type labels and illustrations are set in **Add device / Edit device**; existing inventory needs no conversion, and the original CSV structure stays the same.

## SSH access and keys

Use **Verify this server** by **Connect** for first-time verification help. Compare the fingerprint shown in the external terminal with one from the server console or your administrator before accepting it. If the terminal reports a changed identity, stop and investigate. If asked for your account password after verifying the server, type it in that same terminal; the characters are hidden.

DrakOps does not read or store private keys or passwords. If you can already connect, skip **Set up SSH**. Tailscale SSH doesn’t use the public key reviewed here; regular OpenSSH over a Tailscale address can still use one. **Set up SSH** lets you check the exact host, port and account, independently verify the server fingerprint, and choose an OpenSSH .pub file. It opens **Set up key login**: use the suggested public key or choose another, then review the exact destination and its public-key fingerprint. Drag the window corner to resize setup. Host verification and manual help are under **Other ways to set up**. For another public file, use **Choose a different key** to reveal native Browse or the manual path. **Copy reviewed public key** copies only the approved snapshot for manual enrollment through an independently verified connection or an authorized administrator. Copy itself does not change a server. On supported Linux and macOS clients, **Install key in trusted terminal** hands only the reviewed snapshot to system ssh-copy-id in an external terminal. Verify the host first, confirm the key is not already installed for this account, and use this only for standard SSH access. If access needs restrictions or you are unsure, use the manual steps. Repeating the standard installer can append a duplicate. On Windows, **Install key in trusted terminal** supports a Linux or Unix server with standard OpenSSH and a matching local private-key file beside the reviewed .pub. Confirm the destination/account and that standard setup is appropriate. It opens an independent Windows terminal, checks for working key login, and only if needed requests that the reviewed public key be added to the selected account. Enter any private-key passphrase or server password in the native OpenSSH terminal, never DrakOps. OpenSSH proves the local file pairs with the reviewed public key before adding anything. Existing entries are kept, and an existing entry for that key is not added again. The terminal reports whether a fresh key-only check confirmed the reviewed fingerprint; a key that needs to be unlocked or a restricted account may not pass that check. An interrupted setup can leave the key installed. Do not treat a closed terminal as success. This Windows workflow uses standard direct OpenSSH settings and an already trusted host. The target must use normal account-owned `.ssh/authorized_keys` storage in an owned, non-shared POSIX directory hierarchy. Use manual setup for jump hosts, aliases that rely on SSH configuration, nonstandard permissions or key locations, or configurations that don’t accept an OpenSSH exec shell. Unix-client handoff does not prove selected-key authentication. DrakOps never reads or stores passwords or private keys. Keep existing keys and authentication methods. The wizard requires additional explicit confirmation for an existing root account.

## Themes and provenance

Theme renderer and palette source: [DLMS Learning Lab at 6b54e348](https://github.com/drakahari/dlms_learning_lab/tree/6b54e34869e031e50d6cf0854cce3402f65c6fd4). The complete inventory contains Learning Lab Navy, 5 stock DLMS themes and 22 Omarchy v4.0.4 themes. See [the upstream theme source documentation](docs/THEMES.md) and the two MIT notices under `assets/themes/`. Only theme code and color data were imported. No OS theming files or lesson content are applied or shipped.


### CSV Browse and terminal tabs

In **Import CSV**, choose **Browse for CSV…** or enter a path manually. Selecting a file fills the path only. Use **Review CSV** to read the file without blocking the window and confirm each SSH username, address, and port before adding devices; canceling Browse leaves the path and review unchanged.

**Connect** opens an embedded tab for the device and its selected endpoint. Selecting the same live user, host and port returns to the existing tab. Use the arrow on **Connect**, then **Open new SSH tab**, to start a separate session to that same endpoint. Other connections continue running. Check the active tab's displayed user, host, port and route before typing. Click the terminal to type after switching tabs. **Disconnect** stops the active connection; use **×** on any tab to close just that session. Up to eight tabs can be open, and closing DrakOps disconnects all embedded sessions. External terminals remain independent.


If another DrakOps window changed the saved inventory, the app refuses to overwrite it. Choose **Reload saved inventory** and retry. Open drafts stay available; if the same device was edited elsewhere, review the newly saved values before starting a new edit. Do not run older versions against the same inventory at the same time: they do not participate in the newer save lock.

### Connection routes

Connect shows the destination next to the action before starting SSH. By default it uses the saved host, or the saved Tailscale address then LAN address if there is no separate saved host. When a distinct alternate address is available, **Change route** lets you select it explicitly for this device for the current run. A connection failure does not switch addresses automatically, and changing route does not redirect an open tab.
