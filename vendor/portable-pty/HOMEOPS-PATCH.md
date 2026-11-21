# HomeOps downstream patch of portable-pty 0.9.0

Source: the checksum-locked crates.io portable-pty 0.9.0 archive, upstream WezTerm.
License is reproduced in LICENSE.md.

HomeOps does not interpret arbitrary terminal reports or generate responses to
remote output. On Windows remove PSEUDOCONSOLE_INHERIT_CURSOR so creating a new,
blank pseudoconsole never waits for a host terminal cursor-position report.
Also use the built-in Windows ConPTY implementation from kernel32.dll only;
HomeOps does not ship or load an adjacent conpty.dll. Both changes are in
src/win/psuedocon.rs. Review when updating upstream.
On Unix, the master writer no longer sends a newline and VEOF on drop. Closing
a session must not submit an unfinished line. The DrakOps terminal supervisor
terminates and reaps its child and closes remaining PTY handles; intentional
typed Enter still writes normally. See src/unix.rs and its local PTY fixture.
