//! Headless Blender as a subprocess: only the editor-side pipeline needs it.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::BlenderError;

/// The export script, embedded so the pipeline works outside the source tree.
pub const EXPORT_SCRIPT: &str = include_str!("../scripts/export_gltf.py");

/// Lines of Blender output kept in error messages.
const OUTPUT_TAIL_LINES: usize = 40;

/// How to run Blender. Defaults to `$STRUCTION_BLENDER`, else `blender` on `PATH`,
/// else (macOS) the newest `Blender*.app` in `/Applications` or `~/Applications`.
#[derive(Debug, Clone)]
pub struct Blender {
    pub executable: PathBuf,
    pub timeout: Duration,
}

impl Default for Blender {
    fn default() -> Self {
        let app_dirs = if cfg!(target_os = "macos") {
            let home =
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications"));
            [Some(PathBuf::from("/Applications")), home]
                .into_iter()
                .flatten()
                .collect()
        } else {
            Vec::new()
        };
        Self {
            executable: find_executable(
                std::env::var_os("STRUCTION_BLENDER"),
                std::env::var_os("PATH"),
                &app_dirs,
            ),
            timeout: Duration::from_secs(300),
        }
    }
}

/// The override if set, else `blender` when it is on `path`, else the executable
/// of the newest `Blender*.app` bundle in `app_dirs`, else plain `blender`.
fn find_executable(
    override_path: Option<OsString>,
    path: Option<OsString>,
    app_dirs: &[PathBuf],
) -> PathBuf {
    if let Some(executable) = override_path {
        return executable.into();
    }
    let name = PathBuf::from("blender");
    if on_path(&name, path) {
        return name;
    }
    // Newest version first; an unversioned "Blender.app" is the installer's
    // current one, and earlier directories win ties.
    app_dirs
        .iter()
        .enumerate()
        .filter_map(|(index, dir)| Some((index, std::fs::read_dir(dir).ok()?)))
        .flat_map(|(index, entries)| entries.map(move |entry| (index, entry)))
        .filter_map(|(index, entry)| {
            let entry = entry.ok()?;
            let file_name = entry.file_name().into_string().ok()?;
            let version = file_name.strip_prefix("Blender")?.strip_suffix(".app")?;
            let version: Vec<u32> = if version.is_empty() {
                vec![u32::MAX]
            } else {
                version
                    .trim()
                    .split('.')
                    .map(|part| part.parse().ok())
                    .collect::<Option<_>>()?
            };
            let executable = entry.path().join("Contents/MacOS/Blender");
            is_executable(&executable).then_some(((version, std::cmp::Reverse(index)), executable))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map_or(name, |(_, executable)| executable)
}

fn on_path(name: &Path, path: Option<OsString>) -> bool {
    path.is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(name)))
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

impl Blender {
    /// Whether the executable exists (a path, or a name found on `PATH`).
    pub fn is_available(&self) -> bool {
        if self.executable.components().count() > 1 {
            return is_executable(&self.executable);
        }
        on_path(&self.executable, std::env::var_os("PATH"))
    }

    /// Runs a generator script that builds an asset from code and saves it to
    /// `output` (`.blend`, or glTF when the script exports one). The script
    /// receives an absolute path after `--`: a sibling of `output` with the same
    /// extension, renamed over `output` on success so watchers never see a
    /// missing or partial file.
    pub fn run_generator(&self, script: &Path, output: &Path) -> Result<String, BlenderError> {
        let output = absolute(output);
        let name = output.file_name().unwrap_or_default().to_string_lossy();
        let building = output.with_file_name(format!(".building-{name}"));
        let _ = std::fs::remove_file(&building);
        let log = self.run_script(&absolute(script), &[building.clone().into()])?;
        if !building.is_file() {
            return Err(BlenderError::NoOutput { output: log });
        }
        std::fs::rename(&building, &output).map_err(|source| BlenderError::Move {
            path: output.clone(),
            source,
        })?;
        Ok(log)
    }

    /// Converts a `.blend` or glTF file to GLB with engine axes (+Y up), optionally
    /// generating UVs for meshes without them.
    pub fn export_gltf(
        &self,
        source: &Path,
        output: &Path,
        generate_uvs: bool,
    ) -> Result<(), BlenderError> {
        let dir = tempfile::tempdir().map_err(|source| self.spawn_error(source))?;
        let script = dir.path().join("export_gltf.py");
        std::fs::write(&script, EXPORT_SCRIPT).map_err(|source| self.spawn_error(source))?;
        let _ = std::fs::remove_file(output);

        let mut args = vec![absolute(source).into(), absolute(output).into()];
        if generate_uvs {
            args.push("--generate-uvs".into());
        }
        let log = self.run_script(&script, &args)?;
        if !output.is_file() {
            return Err(BlenderError::NoOutput { output: log });
        }
        Ok(())
    }

    /// Runs `script` in a fresh background Blender (factory settings, no
    /// auto-run scripts from files) with `args` after `--`. Returns the output log.
    pub fn run_script(&self, script: &Path, args: &[OsString]) -> Result<String, BlenderError> {
        let mut child = Command::new(&self.executable)
            .args([
                "--background",
                "--factory-startup",
                "--disable-autoexec",
                "--python-exit-code",
                "1",
                "--python",
            ])
            .arg(script)
            .arg("--")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| self.spawn_error(source))?;

        // Drain both pipes on threads so a chatty Blender never blocks on a full pipe.
        let stdout = Drain::start(child.stdout.take());
        let stderr = Drain::start(child.stderr.take());
        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let (stdout, stderr) = (stdout.finish(), stderr.finish());
                    if status.success() {
                        return Ok(tail(&format!("{stdout}{stderr}")));
                    }
                    return Err(BlenderError::Failed {
                        status,
                        output: format!("{}\n{}", tail(&stderr), tail(&stdout)),
                    });
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Grandchildren may still hold the pipes open: take what arrived.
                    return Err(BlenderError::Timeout {
                        timeout: self.timeout,
                        output: tail(&format!("{}{}", stdout.partial(), stderr.partial())),
                    });
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(source) => return Err(self.spawn_error(source)),
            }
        }
    }

    fn spawn_error(&self, source: std::io::Error) -> BlenderError {
        BlenderError::Spawn {
            executable: self.executable.clone(),
            source,
        }
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

struct Drain {
    thread: thread::JoinHandle<()>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Drain {
    fn start(pipe: Option<impl Read + Send + 'static>) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = bytes.clone();
        let thread = thread::spawn(move || {
            let Some(mut pipe) = pipe else { return };
            let mut chunk = [0; 8192];
            while let Ok(read @ 1..) = pipe.read(&mut chunk) {
                sink.lock().unwrap().extend_from_slice(&chunk[..read]);
            }
        });
        Self { thread, bytes }
    }

    fn finish(self) -> String {
        let Self { thread, bytes } = self;
        let _ = thread.join();
        String::from_utf8_lossy(&bytes.lock().unwrap()).into_owned()
    }

    fn partial(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().unwrap()).into_owned()
    }
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(OUTPUT_TAIL_LINES)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_executable_is_reported() {
        let blender = Blender {
            executable: "/nonexistent/blender".into(),
            timeout: Duration::from_secs(5),
        };
        assert!(!blender.is_available());
        let error = blender
            .export_gltf(Path::new("a.blend"), Path::new("a.glb"), false)
            .unwrap_err();
        assert!(matches!(error, BlenderError::Spawn { .. }), "{error}");
    }

    #[test]
    fn finds_blender_by_override_path_and_app_bundle() {
        use std::os::unix::fs::PermissionsExt;
        let executable = |path: &Path| {
            std::fs::write(path, "").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("Applications");
        let home_apps = dir.path().join("home/Applications");
        for (root, bundle) in [
            (&apps, "Blender 9.2.app"),
            (&apps, "Blender 10.1.app"),
            (&home_apps, "Blender 10.1.app"),
            (&home_apps, "Blender 4.2.app"),
        ] {
            let macos = root.join(bundle).join("Contents/MacOS");
            std::fs::create_dir_all(&macos).unwrap();
            executable(&macos.join("Blender"));
        }
        // Not a Blender bundle, and a bundle without its executable.
        std::fs::create_dir_all(apps.join("Other.app/Contents/MacOS")).unwrap();
        std::fs::create_dir_all(apps.join("Blender 99.app")).unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let dirs = [apps.clone(), home_apps.clone()];

        assert_eq!(
            find_executable(Some("/opt/b".into()), Some(bin.clone().into()), &dirs),
            PathBuf::from("/opt/b")
        );
        // 10.1 beats 9.2 numerically; /Applications wins the tie.
        assert_eq!(
            find_executable(None, Some(bin.clone().into()), &dirs),
            apps.join("Blender 10.1.app/Contents/MacOS/Blender")
        );
        let macos = home_apps.join("Blender.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        executable(&macos.join("Blender"));
        assert_eq!(
            find_executable(None, Some(bin.clone().into()), &dirs),
            macos.join("Blender")
        );
        assert_eq!(find_executable(None, None, &[]), PathBuf::from("blender"));
        // A non-executable `blender` on PATH does not count.
        std::fs::write(bin.join("blender"), "").unwrap();
        assert_ne!(
            find_executable(None, Some(bin.clone().into()), &dirs),
            PathBuf::from("blender")
        );
        executable(&bin.join("blender"));
        assert_eq!(
            find_executable(None, Some(bin.into()), &dirs),
            PathBuf::from("blender")
        );
    }

    #[test]
    fn failures_and_timeouts_carry_output() {
        let dir = tempfile::tempdir().unwrap();
        // A fake Blender: fails loudly, or hangs, depending on its first argument.
        let fake = dir.path().join("fake-blender");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor a; do last=$a; done\n\
             if [ \"$last\" = hang ]; then sleep 10; fi\n\
             echo 'Traceback: boom' >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let blender = Blender {
            executable: fake,
            timeout: Duration::from_millis(300),
        };
        assert!(blender.is_available());

        let error = blender
            .run_script(Path::new("script.py"), &["fail".into()])
            .unwrap_err();
        assert!(matches!(error, BlenderError::Failed { .. }));
        assert!(error.to_string().contains("Traceback: boom"), "{error}");

        let started = Instant::now();
        let error = blender
            .run_script(Path::new("script.py"), &["hang".into()])
            .unwrap_err();
        assert!(matches!(error, BlenderError::Timeout { .. }), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
