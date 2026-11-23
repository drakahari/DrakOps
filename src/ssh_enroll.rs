//! Guided Windows client enrollment for existing standard POSIX OpenSSH accounts.
//! Only validated public key data and the saved account are sent to the remote shell.
use crate::book::{Device, Route};

const REMOTE: &str = r#"PATH=/usr/bin:/bin; export PATH
set -eu
umask 077
unsafe_path() { printf '%s\n' 'Unsafe or nonstandard SSH key storage; use manual setup.' >&2; exit 1; }
account=$(id -un)
uid=$(id -u)
[ "$account" = "$3" ] || unsafe_path
case ${HOME-} in /|*//*|*/) unsafe_path;; /*) ;; *) unsafe_path;; esac
safe_dir() {
    [ -d "$1" ] && [ ! -L "$1" ] &&
    [ "$(find "$1" -prune -type d ! -perm -0020 ! -perm -0002 \( -uid "$uid" -o -uid 0 \) -print)" = "$1" ]
}
# Walk only non-symlink directories, parent first. An unrelated UID cannot
# swap a checked component in a non-group/other-writable trusted parent.
cd /
safe_dir . || unsafe_path
rest=${HOME#/}
while [ -n "$rest" ]; do
    component=${rest%%/*}
    case $component in ''|.|..) unsafe_path;; esac
    safe_dir "./$component" || unsafe_path
    cd -P "./$component"
    if [ "$rest" = "$component" ]; then rest=; else rest=${rest#*/}; fi
done
[ "$(find . -prune -uid "$uid" -print)" = . ] || unsafe_path
if [ ! -e .ssh ] && [ ! -L .ssh ]; then mkdir -m 700 .ssh; fi
safe_dir ./.ssh && [ "$(find ./.ssh -prune -uid "$uid" -print)" = ./.ssh ] || unsafe_path
cd -P .ssh
mkdir -m 700 .drakops-enroll.lock || { printf '%s\n' 'Could not acquire the key setup lock.' >&2; exit 1; }
trap 'rmdir .drakops-enroll.lock' 0
trap 'exit 1' HUP INT TERM
[ ! -L authorized_keys ] || unsafe_path
if [ -e authorized_keys ]; then
    [ -f authorized_keys ] && [ "$(find ./authorized_keys -type f -links 1 -uid "$uid" ! -perm -0020 ! -perm -0002 -print)" = ./authorized_keys ] || unsafe_path
    # Match existing (even restricted) affirmative entries, not commented-out keys.
    if awk -v algorithm="$1" -v blob="$2" '
        # The initial options field can have spaces within a quoted value.
        # Inspect only the plain type/blob or the fields after one options
        # field, so a mention in a comment cannot suppress enrollment.
        function next_field(input, i, ch, quoted, escaped) {
            sub(/^[[:space:]]+/, "", input)
            for (i=1; i<=length(input); ++i) {
                ch=substr(input,i,1)
                if (escaped) { escaped=0; continue }
                if (quoted && ch=="\\") { escaped=1; continue }
                if (ch=="\"") { quoted=!quoted; continue }
                if (!quoted && ch ~ /[[:space:]]/) break
            }
            remainder=substr(input,i)
            return quoted ? "" : substr(input,1,i-1)
        }
        /^[[:space:]]*#/ { next }
        {
            first=next_field($0); second=next_field(remainder)
            if (first==algorithm && second==blob) found=1
            else if (first!="" && second==algorithm && next_field(remainder)==blob) found=1
        }
        END { exit !found }
    ' authorized_keys; then
        printf '%s\n' 'Public key already present; left authorized_keys unchanged.'
        exit 0
    else
        [ "$?" = 1 ] || exit 1
    fi
    # An existing file may not end in a newline. Additional blank lines are harmless.
    printf '\n%s %s\n' "$1" "$2" >> authorized_keys
else
    (set -C; printf '%s %s\n' "$1" "$2" > authorized_keys)
fi
printf '%s\n' 'Added the public key.'
"#;

pub(super) fn enroll_args(device:&Device, route:Route, key:&str)->Result<Vec<String>,String>{
    let (kind,blob)=key.split_once(' ').ok_or("The reviewed key is invalid.")?;
    if !matches!(kind,"ssh-ed25519"|"ssh-rsa"|"ecdsa-sha2-nistp256"|"ecdsa-sha2-nistp384"|"ecdsa-sha2-nistp521")
        || blob.len()<32 || blob.len()>16384 || !blob.bytes().all(|b|b.is_ascii_alphanumeric() || b"+/=".contains(&b)) {
        return Err("The reviewed key is invalid.".into());
    }
    let mut args=direct_args();
    args.extend(["-oPubkeyAuthentication=no","-oPreferredAuthentications=keyboard-interactive,password",
        "-oNumberOfPasswordPrompts=1"].map(str::to_owned));
    args.extend(device.ssh_args(route)?);
    args.push(format!("/bin/sh -c {} drakops-enroll '{kind}' '{blob}' {}", shell_quote(REMOTE),shell_quote(&device.user)));
    Ok(args)
}

fn shell_quote(arg:&str)->String {
    format!("'{}'",arg.replace('\'',"'\"'\"'"))
}

fn direct_args()->Vec<String> {
    ["-T","-F","none","-oStrictHostKeyChecking=yes","-oCheckHostIP=no","-oUpdateHostKeys=no",
        "-oControlMaster=no","-oControlPath=none","-oPermitLocalCommand=no","-oClearAllForwardings=yes",
        "-oForwardAgent=no","-oForwardX11=no","-oConnectTimeout=10"].map(str::to_owned).into()
}

pub(super) fn login_args(device:&Device, route:Route, identity:&str)->Result<Vec<String>,String>{
    let mut args=direct_args();
    args.extend(["-oBatchMode=yes","-oPreferredAuthentications=publickey","-oPasswordAuthentication=no",
        "-oKbdInteractiveAuthentication=no","-oIdentitiesOnly=yes","-oCertificateFile=none","-v","-i"].map(str::to_owned));
    args.push(identity.into());
    args.extend(device.ssh_args(route)?);
    args.push("/bin/true".into());
    Ok(args)
}

// Quote one argument for the Windows CRT/CreateProcess argv convention. Not a
// shell string: ProcessStartInfo receives it directly for system OpenSSH.
fn windows_argv(args:&[String])->String{
    args.iter().map(|arg|{
        let mut result=String::from("\"");
        let mut slashes=0;
        for ch in arg.chars(){
            if ch=='\\' {slashes+=1;continue;}
            result.extend(std::iter::repeat_n('\\',if ch=='"' {slashes*2+1}else{slashes}));
            slashes=0;
            result.push(ch);
        }
        result.extend(std::iter::repeat_n('\\',slashes*2));
        result.push('"');
        result
    }).collect::<Vec<_>>().join(" ")
}

const POWERSHELL_TRANSPORT: &str = r#"
function Decode-Data([string]$Encoded) {
    return [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Encoded))
}
function Start-Native([string]$Executable, [string]$Arguments, [bool]$CapturePublicOutput) {
    $options = New-Object System.Diagnostics.ProcessStartInfo
    $options.FileName = $Executable
    $options.Arguments = $Arguments
    $options.UseShellExecute = $false
    $options.RedirectStandardOutput = $CapturePublicOutput
    return [System.Diagnostics.Process]::Start($options)
}
"#;

pub(super) fn powershell_script(ssh:&str, keygen:&str, verify_local:&[String], expected_public:&str, install:&[String], test:&[String], fingerprint:&str)->String{
    // PowerShell parses both ASCII and typographic quotation marks. Substitute
    // only ASCII base64 here; decode each fixed data field into a .NET string.
    let ssh=encode_bytes(ssh.as_bytes());
    let keygen=encode_bytes(keygen.as_bytes());
    let verify_local=encode_bytes(windows_argv(verify_local).as_bytes());
    let expected_public=encode_bytes(expected_public.as_bytes());
    let install=encode_bytes(windows_argv(install).as_bytes());
    let verify=encode_bytes(windows_argv(test).as_bytes());
    let fingerprint=encode_bytes(fingerprint.as_bytes());
    format!(r#"
$ErrorActionPreference = 'Stop'
{POWERSHELL_TRANSPORT}
$ssh = Decode-Data '{ssh}'
$keygen = Decode-Data '{keygen}'
$verifyLocal = Decode-Data '{verify_local}'
$expectedPublic = Decode-Data '{expected_public}'
$install = Decode-Data '{install}'
$verify = Decode-Data '{verify}'
$fingerprint = Decode-Data '{fingerprint}'
function Confirm-LocalPair {{
    $p = Start-Native -Executable $keygen -Arguments $verifyLocal -CapturePublicOutput $true
    try {{
        $publicOutput = $p.StandardOutput.ReadToEnd()
        $p.WaitForExit()
        $fields = $publicOutput.Trim() -split '\s+'
        return ($p.ExitCode -eq 0 -and $fields.Length -ge 2 -and ($fields[0]+' '+$fields[1]) -ceq $expectedPublic)
    }} finally {{
        $p.Dispose()
    }}
}}
function Invoke-VerifiedKey([string]$Arguments, [string]$Fingerprint) {{
    $log = [IO.Path]::Combine([IO.Path]::GetTempPath(), 'drakops-' + [Guid]::NewGuid().ToString('N') + '.log')
    # -E logs OpenSSH's own diagnostics rather than parsing server stderr.
    [IO.File]::Open($log, [IO.FileMode]::CreateNew).Dispose()
    try {{
        $p = Start-Native -Executable $ssh -Arguments ('-E "' + $log + '" ' + $Arguments) -CapturePublicOutput $false
        try {{
            $p.WaitForExit()
            if ($p.ExitCode -ne 0 -or (Get-Item -LiteralPath $log).Length -gt 65536) {{ return $false }}
            $diagnostics = [IO.File]::ReadAllText($log)
            $accept = '(?m:^debug1: Server accepts key: [^\r\n]*[ \t]' + [regex]::Escape($Fingerprint) + '(?:[ \t]+explicit)?(?:[ \t]+agent)?\r?$)'
            $auth = '(?m:^(?:Authenticated to [^\r\n]+ using "publickey"\.|debug1: Authentication succeeded \(publickey\)\.)\r?$)'
            return ($diagnostics -cmatch $accept -and $diagnostics -cmatch $auth)
        }} finally {{
            $p.Dispose()
        }}
    }} finally {{
        Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
    }}
}}
try {{
    $verified = Invoke-VerifiedKey -Arguments $verify -Fingerprint $fingerprint
    if ($verified) {{
        Write-Host 'Key login already works with this public key. No changes made.'
    }} elseif (-not (Confirm-LocalPair)) {{
        Write-Host 'OpenSSH could not confirm that the selected public key matches the local private key. Nothing was installed.'
    }} else {{
        Write-Host 'Use your SSH account password in this terminal if OpenSSH asks.'
        Write-Host 'If the host key is unknown or has changed, stop and verify it separately.'
        $p = Start-Native -Executable $ssh -Arguments $install -CapturePublicOutput $false
        try {{ $p.WaitForExit(); $code = $p.ExitCode }} finally {{ $p.Dispose() }}
        if ($code -ne 0) {{
            Write-Host 'Setup did not finish cleanly. The key might already have been added.'
            Write-Host 'No key login was confirmed. Review the OpenSSH message above before retrying.'
        }} else {{
            $verified = Invoke-VerifiedKey -Arguments $verify -Fingerprint $fingerprint
            if ($verified) {{
                Write-Host 'Verified: a fresh connection accepted this public key without an account password.'
            }} else {{
                Write-Host 'The setup command finished, but key login was not verified. Try a new connection and check the matching private key or SSH agent.'
            }}
        }}
    }}
}} catch {{
    Write-Host 'Could not confirm setup. Check the terminal output before retrying. Use the manual option if needed.'
}}
"#)
}

pub(super) fn encoded_powershell(text:&str)->String{
    let bytes:Vec<_>=text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    encode_bytes(&bytes)
}
fn encode_bytes(bytes:&[u8])->String{
    const ALPHABET:&[u8;64]=b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output=String::with_capacity(bytes.len().div_ceil(3)*4);
    for chunk in bytes.chunks(3){
        let (a,b,c)=(chunk[0],*chunk.get(1).unwrap_or(&0),*chunk.get(2).unwrap_or(&0));
        output.push(ALPHABET[(a>>2) as usize] as char);
        output.push(ALPHABET[(((a&3)<<4)|(b>>4)) as usize] as char);
        output.push(if chunk.len()>1 {ALPHABET[(((b&15)<<2)|(c>>6)) as usize] as char}else{'='});
        output.push(if chunk.len()>2 {ALPHABET[(c&63) as usize] as char}else{'='});
    }
    output
}

#[cfg(test)]
mod tests{
    use super::*;
    const KEY:&str="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const OTHER:&str="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB";
    fn target()->Device{
        Device{alias:"fixture".into(),host:"192.0.2.23".into(),user:"fixture".into(),port:2222,..Default::default()}
    }
    #[test]
    fn guided_enrollment_pins_direct_destination_and_public_only_payload(){
        let args=enroll_args(&target(),Route::Saved,KEY).unwrap();
        assert!(args.starts_with(&["-T".into(),"-F".into(),"none".into(),"-oStrictHostKeyChecking=yes".into()]));
        assert!(args.contains(&"-oPubkeyAuthentication=no".into()));
        assert_eq!(&args[args.len()-7..args.len()-1],["-l","fixture","-p","2222","--","192.0.2.23"]);
        assert!(args.last().unwrap().contains(&format!("'{}' '{}'",KEY.split_once(' ').unwrap().0, KEY.split_once(' ').unwrap().1)));
        assert!(enroll_args(&target(),Route::Saved,&format!("{KEY}'")).is_err());
        assert!(enroll_args(&target(),Route::Saved,&format!("{KEY}\n")).is_err());
        let invalid=Device{user:"unsafe name".into(),..target()};
        assert!(enroll_args(&invalid,Route::Saved,KEY).is_err());
        let verify=login_args(&target(),Route::Saved,r"C:\name with space\.ssh\key").unwrap();
        assert!(verify.contains(&"-oBatchMode=yes".into()));
        assert!(verify.contains(&"-oIdentitiesOnly=yes".into()));
        assert!(verify.contains(&"-oPasswordAuthentication=no".into()));
        let script=powershell_script(r"C:\OpenSSH\ssh.exe",r"C:\OpenSSH\ssh-keygen.exe",&["-y".into()],KEY,&args,&verify,"SHA256:reviewed");
        assert!(script.contains("RedirectStandardOutput = $CapturePublicOutput"));
        assert!(script.contains("-CapturePublicOutput $false"));
        assert!(script.contains("Confirm-LocalPair"));
        assert!(script.contains("'-E \"' + $log"));
        assert!(script.contains(&encode_bytes(b"SHA256:reviewed")));
        assert!(!script.contains("SHA256:reviewed"));
        let unusual=login_args(&target(),Route::Saved,r"C:\Dev’s ‘review’ Ｋｅｙ\identity").unwrap();
        let script=powershell_script(r"C:\OpenSSH\ssh.exe",r"C:\OpenSSH\ssh-keygen.exe",
            &["-y".into(),"-f".into(),r"C:\Dev’s ‘review’ Ｋｅｙ\identity".into()],KEY,&args,&unusual,"SHA256:reviewed");
        assert!(script.is_ascii());
        assert!(script.contains(&encode_bytes(windows_argv(&unusual).as_bytes())));
        assert!(encoded_powershell(&script).len()<28_000);
        let encoded=encoded_powershell("A");
        assert_eq!(encoded,"QQA=");
        assert_eq!(encoded_powershell("abc"),"YQBiAGMA");
        assert_eq!(windows_argv(&[r#"a"b"#.into(),r"C:\one\".into()]),r#""a\"b" "C:\one\\""#);
    }

    /// Optional release check: prints an exactly transported credential-free
    /// PowerShell 5.1 command; local OpenSSH -V has no network or file writes.
    #[test]
    #[ignore = "emits an inline native PowerShell 5.1 transport fixture"]
    fn emit_native_powershell_transport_fixture(){
        let fixture=encode_bytes("C:\\Test’s ‘review’ Ｋｅｙ\\fixture".as_bytes());
        let args=encode_bytes(windows_argv(&["-V".into()]).as_bytes());
        let script=format!(r#"
$ErrorActionPreference='Stop'
{POWERSHELL_TRANSPORT}
$fixture = Decode-Data '{fixture}'
if ([int][char]$fixture[7] -ne 8217 -or -not $fixture.EndsWith('fixture')) {{ exit 87 }}
$ssh = Join-Path $env:SystemRoot 'System32\OpenSSH\ssh.exe'
$fixtureArgs = Decode-Data '{args}'
$p = Start-Native -Executable $ssh -Arguments $fixtureArgs -CapturePublicOutput $false
try {{ $p.WaitForExit(); if ($p.ExitCode -ne 0) {{ exit $p.ExitCode }} }} finally {{ $p.Dispose() }}
Write-Output 'DRAKOPS_NATIVE_TRANSPORT_VERIFIED'
"#);
        println!("DRAKOPS_POWERSHELL_FIXTURE={}",encoded_powershell(&script));
    }

    #[cfg(unix)]
    #[test]
    fn fixed_installer_preserves_entries_is_idempotent_and_rejects_symlinks(){
        use std::{fs,os::unix::fs::{PermissionsExt,symlink},process::Command};
        let root=std::env::var_os("DRAKOPS_TEST_PRIVATE_DIR").map(std::path::PathBuf::from)
            .unwrap_or_else(||std::env::current_dir().unwrap());
        let home=tempfile::tempdir_in(root).unwrap();
        let keys=home.path().join(".ssh/authorized_keys");
        let who=String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap().trim().to_owned();
        let command=enroll_args(&Device{user:who,..target()},Route::Saved,KEY).unwrap().pop().unwrap();
        let invoke=||Command::new("/bin/sh").arg("-c").arg(&command).env("HOME",home.path()).output().unwrap();
        assert!(invoke().status.success());
        assert_eq!(fs::read_to_string(&keys).unwrap(),format!("{KEY}\n"));
        assert_eq!(fs::metadata(&keys).unwrap().permissions().mode()&0o777,0o600);
        fs::write(&keys,format!("{OTHER} fixture")).unwrap(); // deliberately missing final newline
        assert!(invoke().status.success());
        let combined=format!("{OTHER} fixture\n{KEY}\n");
        assert_eq!(fs::read_to_string(&keys).unwrap(),combined);
        assert!(invoke().status.success());
        assert_eq!(fs::read_to_string(&keys).unwrap(),combined);
        let restricted=format!("command=\"echo trusted text\",restrict {KEY} existing\n");
        fs::write(&keys,&restricted).unwrap();
        assert!(invoke().status.success());
        assert_eq!(fs::read_to_string(&keys).unwrap(),restricted);
        let comment=format!("{OTHER} its comment mentions {KEY}\n");
        fs::write(&keys,&comment).unwrap();
        assert!(invoke().status.success());
        assert_eq!(fs::read_to_string(&keys).unwrap(),format!("{comment}\n{KEY}\n"));
        fs::remove_file(&keys).unwrap();
        let outside=home.path().join("outside");
        fs::write(&outside,"untouched").unwrap();
        fs::hard_link(&outside,&keys).unwrap();
        assert!(!invoke().status.success());
        fs::remove_file(&keys).unwrap();
        symlink(&outside,&keys).unwrap();
        assert!(!invoke().status.success());
        assert_eq!(fs::read_to_string(&outside).unwrap(),"untouched");
        fs::remove_file(&keys).unwrap();
        fs::write(&keys,format!("{OTHER} fixture\n")).unwrap();
        fs::set_permissions(&keys,fs::Permissions::from_mode(0o622)).unwrap();
        assert!(!invoke().status.success());
        fs::set_permissions(&keys,fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(home.path(),fs::Permissions::from_mode(0o722)).unwrap();
        assert!(!invoke().status.success());
        fs::set_permissions(home.path(),fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(keys.parent().unwrap(),fs::Permissions::from_mode(0o720)).unwrap();
        assert!(!invoke().status.success());
        fs::set_permissions(keys.parent().unwrap(),fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read_to_string(&keys).unwrap(),format!("{OTHER} fixture\n"));
    }
}
