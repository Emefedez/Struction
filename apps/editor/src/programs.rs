//! Machine-local program selection; paths never enter portable project sources.
use std::path::{Path, PathBuf};

use struction_assets::{Blender, OpenIn};

pub struct Programs {
    pub blender: Blender,
    pub ide_input: String,
    ide: String,
    pub input: String,
    pub error: Option<String>,
    pub status: Option<String>,
    config: Option<PathBuf>,
}

impl Default for Programs {
    fn default() -> Self {
        Self::load(config_path())
    }
}

fn config_path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
    };
    base.map(|base| base.join("struction/programs.json"))
}

impl Programs {
    fn load(config: Option<PathBuf>) -> Self {
        let mut programs = Self {
            blender: Blender::default(),
            ide: default_ide(),
            ide_input: String::new(),
            input: String::new(),
            error: None,
            status: None,
            config,
        };
        if let Some(path) = &programs.config {
            match std::fs::read_to_string(path) {
                Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(value) => {
                        if let Some(ide) = value.get("ide").and_then(|v| v.as_str()) {
                            programs.ide = ide.into();
                        }
                        if let Some(path) = value.get("blender").and_then(|v| v.as_str()) {
                            programs.blender.executable = executable(PathBuf::from(path));
                        }
                    }
                    Err(error) => programs.error = Some(format!("{}: {error}", path.display())),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => programs.error = Some(format!("{}: {error}", path.display())),
            }
        }
        programs.ide_input = programs.ide.clone();
        programs.input = programs.blender.executable.to_string_lossy().into_owned();
        if programs.error.is_none() && !programs.blender.is_available() {
            programs.error = Some(format!(
                "Blender was not found at {}. Choose an executable with Browse…",
                programs.input
            ));
        }
        programs
    }

    /// Select one executable, without shell parsing; spaces in paths remain literal.
    pub fn select(&mut self, path: PathBuf, persist: bool) -> Result<(), String> {
        let path = executable(path);
        let path = if path.components().count() > 1 {
            std::path::absolute(&path).map_err(|e| format!("{}: {e}", path.display()))?
        } else {
            path
        };
        let blender = Blender {
            executable: path,
            ..Blender::default()
        };
        if !blender.is_available() {
            return Err(format!(
                "{} is not an executable file or a program on PATH",
                blender.executable.display()
            ));
        }
        if persist {
            self.save_setting("blender", serde_json::json!(blender.executable))?;
        }
        self.input = blender.executable.to_string_lossy().into_owned();
        self.blender = blender;
        self.error = None;
        self.status = Some(
            if persist {
                "Saved for this machine"
            } else {
                "Selected for this session"
            }
            .into(),
        );
        Ok(())
    }

    fn save_setting(&self, key: &str, value: serde_json::Value) -> Result<(), String> {
        let path = self
            .config
            .as_ref()
            .ok_or("Cannot locate your user settings directory")?;
        let mut settings = match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(e) => return Err(e.to_string()),
        };
        settings
            .as_object_mut()
            .ok_or("Program settings must be an object")?
            .insert(key.into(), value);
        std::fs::create_dir_all(path.parent().expect("settings directory"))
            .map_err(|e| e.to_string())?;
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save_ide(&mut self) -> Result<(), String> {
        ide_arguments(
            &self.ide_input,
            &struction_editor::SourceLocation {
                file: "source.jsonc".into(),
                line: 1,
                column: 1,
            },
        )?;
        self.save_setting("ide", serde_json::json!(self.ide_input))?;
        self.ide = self.ide_input.clone();
        self.error = None;
        self.status = Some("IDE command saved for this machine".into());
        Ok(())
    }

    pub fn open_ide(&self, location: &struction_editor::SourceLocation) -> Result<(), String> {
        let args = ide_arguments(&self.ide, location)?;
        let mut child = std::process::Command::new(&args[0])
            .args(&args[1..])
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|e| {
                format!(
                    "Cannot open {} with {}: {e}",
                    location.file.display(),
                    args[0]
                )
            })?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    pub fn open_in(&self) -> OpenIn {
        let mut open = OpenIn::default();
        open.by_extension.insert(
            "blend".into(),
            vec![self.blender.executable.to_string_lossy().into_owned()],
        );
        open
    }
}

fn default_ide() -> String {
    std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "code --goto {file}:{line}:{column}".into())
}

/// Split the template before substitution: a source path is always one literal argument.
fn ide_arguments(
    template: &str,
    location: &struction_editor::SourceLocation,
) -> Result<Vec<String>, String> {
    let mut args = shlex::split(template).ok_or("Unclosed quotes in IDE command")?;
    if args.first().is_none_or(String::is_empty) {
        return Err("Choose an IDE command, for example code --goto {file}:{line}".into());
    }
    let has_file = args.iter().any(|s| s.contains("{file}"));
    for arg in &mut args {
        *arg = arg
            .replace("{line}", &location.line.to_string())
            .replace("{column}", &location.column.to_string())
            .replace("{file}", &location.file.to_string_lossy());
    }
    if !has_file {
        args.push(location.file.to_string_lossy().into_owned());
    }
    Ok(args)
}

fn executable(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e == "app") {
        path.join("Contents/MacOS/Blender")
    } else {
        path
    }
}

pub fn picker(current: &str) -> Option<PathBuf> {
    let mut picker = rfd::FileDialog::new().set_title("Choose the Blender executable");
    if let Some(parent) = Path::new(current).parent().filter(|p| p.is_dir()) {
        picker = picker.set_directory(parent);
    }
    picker.pick_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_persisted_and_used_for_both_import_and_open_in() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("settings/programs.json");
        let mut programs = Programs::load(Some(config.clone()));
        let executable = std::env::current_exe().unwrap();
        programs.select(executable.clone(), true).unwrap();
        let loaded = Programs::load(Some(config));
        assert_eq!(loaded.blender.executable, executable);
        assert_eq!(
            loaded.open_in().command_for(Path::new("test.blend")),
            [executable.to_str().unwrap()]
        );
        assert!(programs.select(dir.path().join("missing"), true).is_err());
        assert_eq!(programs.blender.executable, executable);
    }

    #[cfg(unix)]
    #[test]
    fn paths_with_spaces_are_one_executable_argument() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Blender custom installation");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &path).unwrap();
        let mut programs = Programs::load(Some(dir.path().join("settings.json")));
        programs.select(path.clone(), true).unwrap();
        assert_eq!(programs.blender.executable, path);
        assert_eq!(
            programs
                .open_in()
                .command_for(Path::new("model with spaces.blend"))
                .len(),
            1
        );
        assert!(programs.blender.is_available());
    }

    #[test]
    fn ide_templates_keep_source_paths_literal_and_settings_independent() {
        let location = struction_editor::SourceLocation {
            file: "/tmp/project with spaces/$(touch nope){line}.jsonc".into(),
            line: 12,
            column: 3,
        };
        assert_eq!(
            ide_arguments(
                "'/custom IDE/code' --goto {file}:{line}:{column}",
                &location
            )
            .unwrap(),
            [
                "/custom IDE/code",
                "--goto",
                "/tmp/project with spaces/$(touch nope){line}.jsonc:12:3"
            ]
        );
        assert_eq!(
            ide_arguments("editor --wait", &location)
                .unwrap()
                .last()
                .unwrap(),
            location.file.to_str().unwrap()
        );
        assert!(ide_arguments("'broken", &location).is_err());
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("programs.json");
        let mut programs = Programs::load(Some(config.clone()));
        programs.ide_input = "code --goto {file}:{line}".into();
        programs.save_ide().unwrap();
        programs
            .select(std::env::current_exe().unwrap(), true)
            .unwrap();
        let loaded = Programs::load(Some(config));
        assert_eq!(loaded.ide, programs.ide_input);
        assert_eq!(loaded.blender.executable, programs.blender.executable);
    }

    #[test]
    fn bundles_resolve_to_the_executable() {
        assert_eq!(
            executable("/Applications/Blender 5.2.app".into()),
            PathBuf::from("/Applications/Blender 5.2.app/Contents/MacOS/Blender")
        );
    }
}
