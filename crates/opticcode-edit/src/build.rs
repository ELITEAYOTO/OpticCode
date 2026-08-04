use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use opticcode_policy::{NetworkIntent, ProcessLaunch, RunProcessAction};
use opticcode_tools::git_state::{capture_git_state, BuildGitReport};
use opticcode_tools::process_runner::{
    run_process_with_cancellation, CancellationToken, ProcessLaunchMode, ProcessRequest,
};

use crate::{
    EditProcessReport, EditRuntimeOptions, EditStageReport, EditStageStatus, EditValidationKind,
};

#[derive(Debug, Clone)]
pub(crate) struct OfflineBuildInvocation {
    pub tool: String,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub launch: ProcessLaunch,
}

#[derive(Debug)]
pub(crate) struct OfflineBuildResult {
    pub build: EditStageReport,
    pub tests: EditStageReport,
    pub process: EditProcessReport,
}

impl OfflineBuildInvocation {
    pub fn policy_action(&self, cwd: &Path, options: &EditRuntimeOptions) -> RunProcessAction {
        RunProcessAction {
            executable: self.executable.clone(),
            arguments: self.arguments.clone(),
            cwd: cwd.to_path_buf(),
            timeout_ms: options.build_timeout.as_millis().min(u128::from(u64::MAX)) as u64,
            output_limit_bytes: options.output_limit_bytes,
            network: NetworkIntent::Denied,
            launch: self.launch,
            environment_allowlist: vec!["JAVA_HOME".to_string()],
        }
    }
}

pub(crate) fn discover_offline_build(
    root: &Path,
    validations: &[EditValidationKind],
) -> Result<OfflineBuildInvocation> {
    let tests = validations.contains(&EditValidationKind::TestOffline);
    if root.join("pom.xml").is_file() {
        let executable = find_installed_executable(&["mvn.cmd", "mvn.exe", "mvn"])
            .context("Maven executable was not found in the installed toolchain")?;
        let mut arguments = vec!["-o".to_string(), "-q".to_string()];
        if tests {
            arguments.push("verify".to_string());
        } else {
            arguments.extend(["-DskipTests".to_string(), "package".to_string()]);
        }
        return Ok(OfflineBuildInvocation {
            tool: "maven".to_string(),
            launch: launch_for(&executable),
            executable,
            arguments,
        });
    }
    if root.join("build.gradle").is_file() || root.join("build.gradle.kts").is_file() {
        let executable = find_installed_executable(&["gradle.bat", "gradle.exe", "gradle"])
            .context("Gradle executable was not found in the installed toolchain")?;
        return Ok(OfflineBuildInvocation {
            tool: "gradle".to_string(),
            launch: launch_for(&executable),
            executable,
            arguments: vec!["--offline".to_string(), "build".to_string()],
        });
    }
    bail!("no supported Maven or Gradle build manifest was found")
}

pub(crate) fn run_offline_build(
    root: &Path,
    invocation: &OfflineBuildInvocation,
    validations: &[EditValidationKind],
    options: &EditRuntimeOptions,
    cancellation: &CancellationToken,
) -> Result<OfflineBuildResult> {
    let before = capture_git_state(root).context("failed to capture pre-build Git state")?;
    let mut request = ProcessRequest::new(&invocation.executable, root);
    request.args = invocation.arguments.iter().map(OsString::from).collect();
    request.timeout = options.build_timeout;
    request.output_limit_bytes = options.output_limit_bytes;
    request.launch_mode = match invocation.launch {
        ProcessLaunch::Direct => ProcessLaunchMode::Direct,
        ProcessLaunch::WindowsCommandScript => ProcessLaunchMode::WindowsCommandScript,
        ProcessLaunch::Shell => bail!("shell build launch is forbidden"),
    };
    for name in ["JAVA_HOME"] {
        if let Some(value) = env::var_os(name) {
            request.environment.push((OsString::from(name), value));
        }
    }
    let result = run_process_with_cancellation(&request, Some(cancellation))?;
    let after = capture_git_state(root).context("failed to capture post-build Git state")?;
    let guard = BuildGitReport::from_snapshots(before, after, true)?;
    let duration_ms = result.duration.as_millis().min(u128::from(u64::MAX)) as u64;
    let process = EditProcessReport {
        tool: invocation.tool.clone(),
        executable: invocation.executable.display().to_string(),
        arguments: invocation.arguments.clone(),
        status: result.status.as_str().to_string(),
        exit_code: result.exit_code,
        duration_ms,
        timed_out: result.timed_out(),
        cancelled: result.cancelled(),
        output_truncated: result.output.output_truncated,
        stdout_tail: tail(&result.stdout, 40),
        stderr_tail: tail(&result.stderr, 40),
    };
    let success = result.success() && !result.output.output_truncated && !guard.strict_violation();
    let failure = if result.output.output_truncated {
        "build output exceeded its configured bound".to_string()
    } else if guard.strict_violation() {
        "build changed tracked files outside the verified proposal".to_string()
    } else if result.cancelled() {
        "offline build was cancelled".to_string()
    } else if result.timed_out() {
        "offline build timed out".to_string()
    } else {
        format!(
            "offline build failed with status {} and exit code {:?}",
            result.status.as_str(),
            result.exit_code
        )
    };
    let build = if success {
        EditStageReport::passed(
            format!("{} offline verification passed", invocation.tool),
            duration_ms,
        )
    } else {
        EditStageReport::failed("offline build failed", duration_ms, failure.clone())
    };
    let tests_requested = validations.contains(&EditValidationKind::TestOffline);
    let tests = if !tests_requested {
        EditStageReport {
            status: EditStageStatus::NotRun,
            duration_ms: 0,
            summary: "tests were not requested by the validated plan".to_string(),
            errors: Vec::new(),
        }
    } else if success {
        EditStageReport::passed("offline test lifecycle passed", duration_ms)
    } else {
        EditStageReport::failed("offline tests did not pass", duration_ms, failure)
    };
    Ok(OfflineBuildResult {
        build,
        tests,
        process,
    })
}

fn find_installed_executable(names: &[&str]) -> Option<PathBuf> {
    let mut roots = Vec::new();
    for variable in ["MAVEN_HOME", "M2_HOME", "GRADLE_HOME"] {
        if let Some(value) = env::var_os(variable) {
            roots.push(PathBuf::from(value).join("bin"));
        }
    }
    if let Some(path) = env::var_os("PATH") {
        roots.extend(env::split_paths(&path));
    }
    roots.into_iter().find_map(|root| {
        names.iter().find_map(|name| {
            let candidate = root.join(name);
            let metadata = fs::symlink_metadata(&candidate).ok()?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return None;
            }
            fs::canonicalize(candidate)
                .ok()
                .map(normalize_verbatim_path)
        })
    })
}

fn normalize_verbatim_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        const PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
        let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if wide.starts_with(PREFIX) {
            return PathBuf::from(std::ffi::OsString::from_wide(&wide[PREFIX.len()..]));
        }
    }
    path
}

fn launch_for(executable: &Path) -> ProcessLaunch {
    let name = executable
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if cfg!(windows) && (name.ends_with(".cmd") || name.ends_with(".bat")) {
        ProcessLaunch::WindowsCommandScript
    } else {
        ProcessLaunch::Direct
    }
}

fn tail(value: &str, limit: usize) -> String {
    let lines = value.lines().collect::<Vec<_>>();
    lines[lines.len().saturating_sub(limit)..].join("\n")
}
