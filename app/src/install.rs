//! Larrier as an installed app (Windows): installed for everyone (the
//! program in Program Files, shared files in ProgramData, needs admin) or
//! just for the current user (everything under the user's profile), with
//! Start menu / Desktop shortcuts and an entry in Settings → Apps that runs
//! Larrier's own uninstaller. Each user's sessions and settings always stay
//! in their own profile (`larrier_core::store::data_root`).
//!
//! The release binary is its own installer: run from anywhere else it
//! offers to install itself (see `ui::setup`).
//!
//! macOS and Linux install programs by copying them: Larrier.app wherever
//! it was put, or the program `install.sh` placed in `~/.local/bin`. Such a
//! copy is [`Kind::App`]; it updates where it is and never shows the setup.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

pub const EXE_NAME: &str = "Larrier.exe";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Larrier";
const PUBLISHER: &str = "Dev4YM";

/// Who an install is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Everyone on this computer (admin).
    Machine,
    /// The current user only.
    User,
}

/// How this copy of Larrier is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Installed(Scope),
    /// A downloaded binary run from anywhere else.
    Portable,
    /// macOS or Linux: installed by copying it (see the module docs).
    App,
    /// Built from source (cargo): updates come from the checkout.
    Dev,
}

/// Where an install puts things.
#[derive(Clone, Debug)]
pub struct Layout {
    pub scope: Scope,
    /// The program.
    pub bin_dir: PathBuf,
    /// Install details and downloaded updates.
    pub shared_dir: PathBuf,
    pub start_menu: PathBuf,
    pub desktop: Option<PathBuf>,
}

impl Layout {
    pub fn exe(&self) -> PathBuf {
        self.bin_dir.join(EXE_NAME)
    }

    fn shortcut(dir: &Path) -> PathBuf {
        dir.join("Larrier.lnk")
    }

    fn registry_root(&self) -> &'static str {
        match self.scope {
            Scope::Machine => "HKLM",
            Scope::User => "HKCU",
        }
    }
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn layout(scope: Scope) -> Layout {
    let local = env_dir("LOCALAPPDATA").or_else(dirs::data_local_dir).unwrap_or_default();
    match scope {
        Scope::Machine => Layout {
            scope,
            bin_dir: env_dir("ProgramW6432").or_else(|| env_dir("ProgramFiles")).unwrap_or_else(|| PathBuf::from(r"C:\Program Files")).join("Larrier"),
            shared_dir: env_dir("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData")).join("Larrier"),
            start_menu: env_dir("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData")).join(r"Microsoft\Windows\Start Menu\Programs"),
            desktop: env_dir("PUBLIC").map(|p| p.join("Desktop")),
        },
        Scope::User => Layout {
            scope,
            bin_dir: local.join("Programs").join("Larrier"),
            shared_dir: local.join("Larrier"),
            start_menu: env_dir("APPDATA").or_else(dirs::config_dir).unwrap_or_default().join(r"Microsoft\Windows\Start Menu\Programs"),
            desktop: dirs::desktop_dir(),
        },
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// How the running copy of Larrier is installed.
pub fn current() -> Kind {
    let Ok(exe) = std::env::current_exe() else { return Kind::Portable };
    kind_of(&exe)
}

pub fn kind_of(exe: &Path) -> Kind {
    let in_target = exe.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target"));
    if cfg!(debug_assertions) || in_target {
        return Kind::Dev;
    }
    if !cfg!(windows) {
        return Kind::App;
    }
    let dir = exe.parent().unwrap_or(exe);
    for scope in [Scope::Machine, Scope::User] {
        if same_path(dir, &layout(scope).bin_dir) {
            return Kind::Installed(scope);
        }
    }
    Kind::Portable
}

/// What an install recorded about itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub scope: Scope,
    pub version: String,
    pub exe: PathBuf,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

fn record_path(l: &Layout) -> PathBuf {
    l.shared_dir.join("install.json")
}

/// An existing install of either kind (newest first: for everyone, then
/// for the current user).
pub fn existing() -> Option<Record> {
    [Scope::Machine, Scope::User].into_iter().find_map(|s| {
        let l = layout(s);
        let r: Record = serde_json::from_str(&std::fs::read_to_string(record_path(&l)).ok()?).ok()?;
        r.exe.exists().then_some(r)
    })
}

/// Whether this process can write where a `scope` install goes (installing
/// for everyone needs admin).
pub fn can_install(scope: Scope) -> bool {
    let dir = layout(scope).bin_dir;
    let probe = dir.join(".larrier-write-test");
    let ok = std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Install the running binary for `scope`. Returns the installed program.
pub fn install(scope: Scope, desktop_shortcut: bool) -> Result<PathBuf> {
    let l = layout(scope);
    let me = std::env::current_exe().context("finding Larrier's own file")?;
    std::fs::create_dir_all(&l.bin_dir).with_context(|| format!("creating {}", l.bin_dir.display()))?;
    std::fs::create_dir_all(&l.shared_dir).with_context(|| format!("creating {}", l.shared_dir.display()))?;
    let exe = l.exe();
    if !same_path(&me, &exe) {
        replace_file(&me, &exe)?;
    }
    shortcut(&Layout::shortcut(&l.start_menu), &exe, &l.bin_dir)?;
    if desktop_shortcut {
        if let Some(d) = &l.desktop {
            shortcut(&Layout::shortcut(d), &exe, &l.bin_dir)?;
        }
    }
    register(&l, &exe, larrier_core::APP_VERSION)?;
    let rec = Record { scope, version: larrier_core::APP_VERSION.to_string(), exe: exe.clone(), installed_at: chrono::Utc::now() };
    std::fs::write(record_path(&l), serde_json::to_vec_pretty(&rec)?)?;
    Ok(exe)
}

/// Tidy up after an update: the program it replaced, and downloaded
/// updates that are now in place (run at startup).
pub fn tidy() {
    let updates = match current() {
        Kind::Installed(scope) => {
            let l = layout(scope);
            let _ = std::fs::remove_file(sibling(&l.exe(), "old"));
            let _ = std::fs::remove_file(sibling(&l.exe(), "new"));
            l.shared_dir.join("updates")
        }
        Kind::App => crate::update::cache_dir(),
        _ => return,
    };
    if let Ok(dirs) = std::fs::read_dir(updates) {
        for d in dirs.flatten() {
            let v = d.file_name().to_string_lossy().to_string();
            if !crate::version::newer(&v, crate::version::VERSION) {
                let _ = std::fs::remove_dir_all(d.path());
            }
        }
    }
}

/// `path` with `.<tag>` added to its name (`Larrier.exe.old`, `larrier.new`).
fn sibling(path: &Path, tag: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{tag}"));
    path.with_file_name(name)
}

/// Copy `from` over `to`, retrying while the old program is still closing.
pub fn replace_file(from: &Path, to: &Path) -> Result<()> {
    let tmp = sibling(to, "new");
    std::fs::copy(from, &tmp).with_context(|| format!("copying Larrier to {}", tmp.display()))?;
    let mut last = None;
    for _ in 0..40 {
        // A running .exe can be renamed but not overwritten.
        let old = sibling(to, "old");
        let _ = std::fs::remove_file(&old);
        if to.exists() {
            if let Err(e) = std::fs::rename(to, &old) {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250));
                continue;
            }
        }
        match std::fs::rename(&tmp, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(anyhow!("could not replace {}: {}", to.display(), last.map(|e| e.to_string()).unwrap_or_default()))
}

/// macOS: the `.app` bundle a program runs from.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Put the bundle `from` in place of the bundle `to` (macOS): the old one
/// moves aside first and is removed once the new one is there.
pub fn replace_bundle(from: &Path, to: &Path) -> Result<()> {
    let old = sibling(to, "old");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(to, &old).with_context(|| format!("moving {} aside", to.display()))?;
    // `ditto` copies a bundle as the Finder does (across volumes, keeping
    // its permissions and signature).
    let copied = Command::new("ditto").arg(from).arg(to).status().map(|s| s.success()).unwrap_or(false);
    if !copied {
        let _ = std::fs::remove_dir_all(to);
        std::fs::rename(&old, to).ok();
        return Err(anyhow!("could not copy the new Larrier.app to {}", to.display()));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// The Settings → Apps entry: name, version, icon, size and the uninstaller.
fn register(l: &Layout, exe: &Path, version: &str) -> Result<()> {
    let size_kb = std::fs::metadata(exe).map(|m| m.len() / 1024).unwrap_or(0);
    let exe_s = exe.display().to_string();
    let script = format!(
        "$k = '{root}:\\{key}'; New-Item -Path $k -Force | Out-Null; \
         $v = @{{ DisplayName = 'Larrier'; DisplayVersion = '{version}'; Publisher = '{publisher}'; DisplayIcon = '{exe},0'; \
         InstallLocation = '{dir}'; UninstallString = '\"{exe}\" --uninstall'; QuietUninstallString = '\"{exe}\" --uninstall --quiet'; \
         URLInfoAbout = 'https://github.com/Dev4YM/Larrier' }}; \
         foreach ($n in $v.Keys) {{ New-ItemProperty -Path $k -Name $n -Value $v[$n] -PropertyType String -Force | Out-Null }}; \
         foreach ($n in 'NoModify','NoRepair') {{ New-ItemProperty -Path $k -Name $n -Value 1 -PropertyType DWord -Force | Out-Null }}; \
         New-ItemProperty -Path $k -Name EstimatedSize -Value {size_kb} -PropertyType DWord -Force | Out-Null",
        root = l.registry_root(),
        key = UNINSTALL_KEY,
        version = ps_quote(version),
        publisher = PUBLISHER,
        exe = ps_quote(&exe_s),
        dir = ps_quote(&l.bin_dir.display().to_string()),
    );
    powershell(&script).context("registering Larrier in Settings → Apps")
}

/// Record a new version in the install's Settings → Apps entry and record.
pub fn set_version(scope: Scope, version: &str) -> Result<()> {
    let l = layout(scope);
    powershell(&format!(
        "Set-ItemProperty -Path '{}:\\{}' -Name DisplayVersion -Value '{}'",
        l.registry_root(),
        UNINSTALL_KEY,
        ps_quote(version)
    ))?;
    if let Ok(t) = std::fs::read_to_string(record_path(&l)) {
        if let Ok(mut r) = serde_json::from_str::<Record>(&t) {
            r.version = version.to_string();
            let _ = std::fs::write(record_path(&l), serde_json::to_vec_pretty(&r)?);
        }
    }
    Ok(())
}

fn shortcut(lnk: &Path, exe: &Path, dir: &Path) -> Result<()> {
    if let Some(parent) = lnk.parent() {
        std::fs::create_dir_all(parent)?;
    }
    powershell(&format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{lnk}'); $s.TargetPath = '{exe}'; $s.WorkingDirectory = '{dir}'; \
         $s.IconLocation = '{exe},0'; $s.Description = 'One window for every coding agent'; $s.Save()",
        lnk = ps_quote(&lnk.display().to_string()),
        exe = ps_quote(&exe.display().to_string()),
        dir = ps_quote(&dir.display().to_string()),
    ))
    .with_context(|| format!("creating the shortcut {}", lnk.display()))
}

/// Remove an install: shortcuts, the Settings → Apps entry, its shared
/// files, and (after Larrier exits) the program. `remove_data` also deletes
/// the current user's sessions and settings.
pub fn uninstall(scope: Scope, remove_data: bool) -> Result<()> {
    let l = layout(scope);
    let _ = std::fs::remove_file(Layout::shortcut(&l.start_menu));
    if let Some(d) = &l.desktop {
        let _ = std::fs::remove_file(Layout::shortcut(d));
    }
    // Shortcuts made for the user while it was a for-everyone install.
    if let Some(d) = dirs::desktop_dir() {
        let _ = std::fs::remove_file(Layout::shortcut(&d));
    }
    powershell(&format!("Remove-Item -Path '{}:\\{}' -Recurse -Force -ErrorAction SilentlyContinue", l.registry_root(), UNINSTALL_KEY))?;
    let _ = std::fs::remove_dir_all(&l.shared_dir);
    if remove_data {
        let _ = std::fs::remove_dir_all(larrier_core::store::data_root());
    }
    // The program can't delete itself while running: a detached shell does
    // it once this process is gone.
    let bin = l.bin_dir.display().to_string();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED: u32 = 0x0000_0008;
        const NO_WINDOW: u32 = 0x0800_0000;
        // Passed to cmd as written: Rust's argument quoting isn't cmd's.
        Command::new("cmd")
            .raw_arg(format!("/C ping 127.0.0.1 -n 4 > nul & rmdir /s /q \"{bin}\""))
            .creation_flags(DETACHED | NO_WINDOW)
            .spawn()
            .context("scheduling the program's removal")?;
    }
    #[cfg(not(windows))]
    let _ = bin;
    Ok(())
}

/// Run Larrier again with administrator rights (Windows asks the user) and
/// wait for it. Returns its exit code.
pub fn run_elevated(exe: &Path, args: &[String]) -> Result<i32> {
    let list = args.iter().map(|a| format!("'{}'", ps_quote(&format!("\"{a}\"")))).collect::<Vec<_>>().join(",");
    let script = format!(
        "try {{ $p = Start-Process -FilePath '{exe}' -ArgumentList @({list}) -Verb RunAs -Wait -PassThru; exit $p.ExitCode }} catch {{ exit 1223 }}",
        exe = ps_quote(&exe.display().to_string()),
    );
    let st = ps_command(&script).status().context("asking for administrator rights")?;
    match st.code() {
        Some(1223) => Err(anyhow!("Administrator rights were not granted")),
        Some(c) => Ok(c),
        None => Err(anyhow!("the elevated Larrier did not report back")),
    }
}

/// Start a helper program without waiting: a plain process, no app of its
/// own (the update hand-over).
pub fn spawn(exe: &Path, args: &[&str]) -> Result<()> {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().with_context(|| format!("starting {}", exe.display()))?;
    Ok(())
}

/// Start a program without waiting (and without this process's console).
/// On macOS a program inside a bundle starts as the app (`open`), so it gets
/// its Dock icon and menu bar.
pub fn launch(exe: &Path, args: &[&str]) -> Result<()> {
    if cfg!(target_os = "macos") {
        if let Some(app) = bundle_of(exe) {
            let mut cmd = Command::new("open");
            cmd.arg("-n").arg(&app);
            if !args.is_empty() {
                cmd.arg("--args").args(args);
            }
            cmd.spawn().with_context(|| format!("starting {}", app.display()))?;
            return Ok(());
        }
    }
    let mut cmd = Command::new(exe);
    cmd.args(args);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().with_context(|| format!("starting {}", exe.display()))?;
    Ok(())
}

fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

fn ps_command(script: &str) -> Command {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn powershell(script: &str) -> Result<()> {
    let out = ps_command(script).output().context("running PowerShell")?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!("{}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_where_it_runs_from() {
        assert_eq!(kind_of(Path::new(r"D:\src\Larrier\target\release\larrier-desktop.exe")), Kind::Dev);
        if !cfg!(debug_assertions) {
            assert_eq!(kind_of(&layout(Scope::User).exe()), Kind::Installed(Scope::User));
            assert_eq!(kind_of(&layout(Scope::Machine).exe()), Kind::Installed(Scope::Machine));
            assert_eq!(kind_of(Path::new(r"C:\Users\me\Downloads\Larrier-0.2.0-windows-x64.exe")), Kind::Portable);
        }
        let user = layout(Scope::User);
        assert!(user.bin_dir.ends_with(r"Programs\Larrier") && user.shared_dir.ends_with("Larrier"));
        assert!(layout(Scope::Machine).shared_dir.to_string_lossy().contains("ProgramData"));
    }
}
