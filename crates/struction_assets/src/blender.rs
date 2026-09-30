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

/// How to run Blender. Defaults to `$STRUCTION_BLENDER` or `blender` on `PATH`.
#[derive(Debug, Clone)]
pub struct Blender {
    pub executable: PathBuf,
    pub timeout: Duration,
}

impl Default for Blender {
    fn default() -> Self {
        Self {
            executable: std::env::var_os("STRUCTION_BLENDER")
                .map_or_else(|| PathBuf::from("blender"), PathBuf::from),
            timeout: Duration::from_secs(300),
        }
    }
}

impl Blender {
    /// Whether the executable exists (a path, or a name found on `PATH`).
    pub fn is_available(&self) -> bool {
        if self.executable.components().count() > 1 {
            return self.executable.is_file();
        }
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(&self.executable).is_file())
        })
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
