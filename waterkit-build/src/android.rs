//! Android platform build utilities.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// Configuration for Android/Kotlin builds.
#[derive(Debug, Clone, Default)]
pub struct AndroidConfig {
    /// Additional classpath entries.
    pub extra_classpath: Vec<PathBuf>,
}

fn executable_candidates(bin_dir: &Path, executable: &str) -> Vec<PathBuf> {
    if cfg!(target_os = "windows") {
        vec![
            bin_dir.join(format!("{executable}.exe")),
            bin_dir.join(format!("{executable}.bat")),
            bin_dir.join(format!("{executable}.cmd")),
            bin_dir.join(executable),
        ]
    } else {
        vec![bin_dir.join(executable)]
    }
}

fn executable_from_home(home: &Path, executable: &str) -> Option<PathBuf> {
    let bin_dir = home.join("bin");
    executable_candidates(&bin_dir, executable)
        .into_iter()
        .find(|candidate| candidate.exists())
}

fn executable_from_path(executable: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .flat_map(|dir| executable_candidates(&dir, executable))
        .find(|candidate| candidate.exists())
}

fn resolve_kotlinc_path() -> PathBuf {
    if let Some(kotlinc) = env::var_os("KOTLINC") {
        let kotlinc_path = PathBuf::from(kotlinc);
        assert!(
            kotlinc_path.exists(),
            "KOTLINC is set to '{}' but that path does not exist",
            kotlinc_path.display()
        );
        return kotlinc_path;
    }

    if let Some(kotlin_home) = env::var_os("KOTLIN_HOME") {
        let kotlin_home = PathBuf::from(kotlin_home);
        if let Some(path) = executable_from_home(&kotlin_home, "kotlinc") {
            return path;
        }
        panic!(
            "KOTLIN_HOME is set to '{}' but no Kotlin compiler was found under '{}/bin'",
            kotlin_home.display(),
            kotlin_home.display()
        );
    }

    if let Some(path) = executable_from_path("kotlinc") {
        return path;
    }

    if cfg!(target_os = "windows")
        && let Some(program_files) = env::var_os("ProgramFiles")
    {
        let android_studio_kotlin_home =
            PathBuf::from(program_files).join("Android/Android Studio/plugins/Kotlin/kotlinc");
        if let Some(path) = executable_from_home(&android_studio_kotlin_home, "kotlinc") {
            return path;
        }
    }

    PathBuf::from("kotlinc")
}

fn resolve_java_path() -> PathBuf {
    if let Some(java_home) = env::var_os("JAVA_HOME") {
        let java_home = PathBuf::from(java_home);
        if let Some(path) = executable_from_home(&java_home, "java") {
            return path;
        }
        panic!(
            "JAVA_HOME is set to '{}' but no Java executable was found under '{}/bin'",
            java_home.display(),
            java_home.display()
        );
    }

    PathBuf::from("java")
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.iter().any(|existing| existing == &candidate) {
        paths.push(candidate);
    }
}

fn kotlin_home_candidates(kotlinc_path: &Path) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(kotlin_home) = env::var_os("KOTLIN_HOME") {
        push_unique_path(&mut homes, PathBuf::from(kotlin_home));
    }

    let compiler_paths = [
        Some(kotlinc_path.to_path_buf()),
        fs::canonicalize(kotlinc_path).ok(),
    ];
    for compiler_path in compiler_paths.into_iter().flatten() {
        let Some(home) = compiler_path
            .parent()
            .and_then(Path::parent)
            .map(PathBuf::from)
        else {
            continue;
        };
        push_unique_path(&mut homes, home.clone());
        push_unique_path(&mut homes, home.join("libexec"));
    }

    homes
}

fn kotlin_stdlib_jars_in_home(kotlin_home: &Path) -> Vec<PathBuf> {
    let lib_dir = kotlin_home.join("lib");
    let Ok(entries) = fs::read_dir(&lib_dir) else {
        return Vec::new();
    };

    let mut jars = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        })
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("kotlin-stdlib"))
        })
        .collect::<Vec<_>>();
    jars.sort();
    jars
}

fn detect_kotlin_stdlib_jars(kotlinc_path: &Path) -> Vec<PathBuf> {
    for kotlin_home in kotlin_home_candidates(kotlinc_path) {
        let jars = kotlin_stdlib_jars_in_home(&kotlin_home);
        if !jars.is_empty() {
            return jars;
        }
    }
    Vec::new()
}

fn command_for_executable(executable: &Path) -> Command {
    Command::new(executable)
}

/// Find the android.jar path from ANDROID_HOME.
///
/// # Returns
/// Path to android.jar, or None if not found.
#[must_use]
pub fn find_android_jar() -> Option<PathBuf> {
    let android_home = env::var("ANDROID_HOME")
        .or_else(|_| env::var("ANDROID_SDK_ROOT"))
        .ok()
        .or_else(|| {
            // Try common location on macOS
            let home = env::var("HOME").ok()?;
            let sdk_path = PathBuf::from(home).join("Library/Android/sdk");
            if sdk_path.exists() {
                Some(sdk_path.to_string_lossy().to_string())
            } else {
                None
            }
        })?;

    let platforms_dir = PathBuf::from(&android_home).join("platforms");

    // Find the highest API level
    let mut best_api = 0u32;
    let mut best_path = None;

    if let Ok(entries) = fs::read_dir(&platforms_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(api_str) = name_str.strip_prefix("android-")
                && let Ok(api) = api_str.parse::<u32>()
                && api > best_api
            {
                let jar = entry.path().join("android.jar");
                if jar.exists() {
                    best_api = api;
                    best_path = Some(jar);
                }
            }
        }
    }

    best_path
}

/// Find the d8.jar path from ANDROID_HOME.
///
/// # Returns
/// Path to d8.jar, or None if not found.
#[must_use]
pub fn find_d8_jar() -> Option<PathBuf> {
    let android_home = env::var("ANDROID_HOME")
        .or_else(|_| env::var("ANDROID_SDK_ROOT"))
        .ok()
        .or_else(|| {
            // Try common location on macOS
            let home = env::var("HOME").ok()?;
            let sdk_path = PathBuf::from(home).join("Library/Android/sdk");
            if sdk_path.exists() {
                Some(sdk_path.to_string_lossy().to_string())
            } else {
                None
            }
        })?;

    let build_tools_dir = PathBuf::from(&android_home).join("build-tools");

    // Find the highest version
    let mut best_version: Option<String> = None;
    let mut best_path = None;

    if let Ok(entries) = fs::read_dir(&build_tools_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();

            // Check if this version has d8.jar
            let d8_path = entry.path().join("lib").join("d8.jar");
            if d8_path.exists() && best_version.as_ref().is_none_or(|v| &name_str > v) {
                best_version = Some(name_str);
                best_path = Some(d8_path);
            }
        }
    }

    best_path
}

/// Compile Kotlin files to DEX for Android.
///
/// This handles:
/// 1. Compiling .kt files to .class using kotlinc
/// 2. Converting .class to .dex using D8
///
/// # Arguments
/// * `kotlin_files` - Slice of relative paths to Kotlin source files
///
/// # Panics
/// Panics if compilation fails or Android SDK is not found.
pub fn build_kotlin(kotlin_files: &[&str]) {
    build_kotlin_with_config(kotlin_files, &AndroidConfig::default());
}

/// Compile Kotlin files to DEX with custom configuration.
///
/// # Arguments
/// * `kotlin_files` - Slice of relative paths to Kotlin source files
/// * `config` - Android build configuration
///
/// # Panics
/// Panics if compilation fails or Android SDK is not found.
pub fn build_kotlin_with_config(kotlin_files: &[&str], config: &AndroidConfig) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Track changes
    for kotlin_file in kotlin_files {
        println!("cargo:rerun-if-changed={kotlin_file}");
    }

    // Find android.jar
    let android_jar = find_android_jar().expect("Failed to find android.jar. Is ANDROID_HOME set?");

    // Compile .kt -> .class using kotlinc
    let classes_dir = out_dir.join("classes");
    fs::create_dir_all(&classes_dir).expect("Failed to create classes directory");

    let kotlinc_executable = resolve_kotlinc_path();
    let kotlin_stdlib_jars = detect_kotlin_stdlib_jars(&kotlinc_executable);
    assert!(
        !kotlin_stdlib_jars.is_empty(),
        "Failed to locate Kotlin standard library jars from KOTLIN_HOME or kotlinc path. \
Set KOTLIN_HOME to your Kotlin compiler directory and ensure '<KOTLIN_HOME>/lib/kotlin-stdlib*.jar' exists."
    );

    let mut classpath_entries =
        Vec::with_capacity(1 + kotlin_stdlib_jars.len() + config.extra_classpath.len());
    classpath_entries.push(android_jar.clone());
    classpath_entries.extend(kotlin_stdlib_jars.iter().cloned());
    classpath_entries.extend(config.extra_classpath.iter().cloned());
    let classpath = env::join_paths(&classpath_entries)
        .expect("Failed to construct Kotlin classpath from AndroidConfig");

    let mut kotlinc = command_for_executable(&kotlinc_executable);
    kotlinc
        .arg("-classpath")
        .arg(&classpath)
        .arg("-d")
        .arg(&classes_dir);

    // Add Kotlin source files
    for kotlin_file in kotlin_files {
        kotlinc.arg(manifest_dir.join(kotlin_file));
    }

    let kotlinc_output = kotlinc.output().unwrap_or_else(|error| {
        panic!(
            "Failed to run Kotlin compiler `{}`: {error}",
            kotlinc_executable.display()
        )
    });

    if !kotlinc_output.status.success() {
        eprintln!(
            "kotlinc stderr: {}",
            String::from_utf8_lossy(&kotlinc_output.stderr)
        );
        panic!("kotlinc compilation failed");
    }

    // Find all .class files recursively
    let mut class_files = Vec::new();
    find_class_files(&classes_dir, &mut class_files);

    assert!(
        !class_files.is_empty(),
        "No .class files generated by kotlinc"
    );

    // Find d8.jar
    let d8_jar = find_d8_jar().expect("Failed to find d8.jar. Is Android build-tools installed?");

    // Convert .class -> .dex using D8
    let java_executable = resolve_java_path();
    let mut java = command_for_executable(&java_executable);
    java.arg("-cp")
        .arg(&d8_jar)
        .arg("com.android.tools.r8.D8")
        .arg("--classpath")
        .arg(&android_jar)
        .arg("--output")
        .arg(&out_dir);

    for stdlib in &kotlin_stdlib_jars {
        java.arg("--classpath").arg(stdlib);
    }

    for cp in &config.extra_classpath {
        java.arg("--classpath").arg(cp);
    }

    for class_file in &class_files {
        java.arg(class_file);
    }

    // Package extra classpath JARs into the generated dex so modules can stay self-contained.
    for cp in &config.extra_classpath {
        if cp.extension().is_some_and(|ext| ext == "jar") {
            java.arg(cp);
        }
    }

    let d8_output = java.output().unwrap_or_else(|error| {
        panic!(
            "Failed to run Java executable `{}` for D8: {error}",
            java_executable.display()
        )
    });
    if !d8_output.status.success() {
        eprintln!("D8 stderr: {}", String::from_utf8_lossy(&d8_output.stderr));
        panic!("D8 dexing failed");
    }
}

fn find_class_files(dir: &Path, results: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                find_class_files(&path, results);
            } else if path.extension().is_some_and(|e| e == "class") {
                results.push(path);
            }
        }
    }
}
