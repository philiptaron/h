//! Generating the shell functions printed by `h-shell-init` and `up-shell-init`.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub const H_USAGE: &str =
    "Usage: eval \"$(h-shell-init [--pushd] [--name NAME] [--git-opts \"OPTIONS\"] [code-root])\"";
pub const UP_USAGE: &str = "Usage: eval \"$(up-shell-init [--pushd])\"";

/// Environment variable holding the default code root.
pub const CODE_ROOT_ENV: &str = "H_CODE_ROOT";
/// Code root used when neither an argument nor [`CODE_ROOT_ENV`] gives one.
pub const DEFAULT_CODE_ROOT: &str = "~/src";

/// The shell that will evaluate the generated code, which decides the completion code emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Unknown,
}

impl Shell {
    /// Identify a shell from a process name, as found in `/proc/<pid>/comm`.
    pub fn from_comm(comm: &str) -> Shell {
        match comm.trim_end_matches('\n') {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            _ => Shell::Unknown,
        }
    }

    /// Identify the shell running this process, which is the one that will `eval` our output.
    pub fn detect_parent() -> Shell {
        let ppid = std::os::unix::process::parent_id();
        std::fs::read_to_string(format!("/proc/{ppid}/comm"))
            .map_or(Shell::Unknown, |comm| Shell::from_comm(&comm))
    }
}

/// Whether `cd` or `pushd` is used to change directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdCommand {
    Cd,
    Pushd,
}

impl CdCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            CdCommand::Cd => "cd",
            CdCommand::Pushd => "pushd",
        }
    }
}

/// Options accepted by `h-shell-init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HInitOptions {
    pub func_name: String,
    pub cd: CdCommand,
    pub git_opts: String,
    pub code_root: Option<OsString>,
}

impl Default for HInitOptions {
    fn default() -> Self {
        HInitOptions {
            func_name: "h".into(),
            cd: CdCommand::Cd,
            git_opts: String::new(),
            code_root: None,
        }
    }
}

/// What `h-shell-init` or `up-shell-init` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command<T> {
    Help,
    Run(T),
}

fn unknown_option(arg: &OsStr) -> String {
    format!("Unknown option: {}", arg.to_string_lossy())
}

/// Parse the arguments to `h-shell-init` (excluding the program name).
pub fn parse_h_init_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Command<HInitOptions>, String> {
    let mut opts = HInitOptions::default();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        let has_value = args.peek().is_some();
        match arg.to_str() {
            Some("--pushd") => opts.cd = CdCommand::Pushd,
            Some("--name") if has_value => {
                opts.func_name = args.next().unwrap().to_string_lossy().into_owned()
            }
            Some("--git-opts") if has_value => {
                opts.git_opts = args.next().unwrap().to_string_lossy().into_owned()
            }
            Some("-h" | "--help") => return Ok(Command::Help),
            _ if !arg.as_encoded_bytes().starts_with(b"-") => opts.code_root = Some(arg),
            _ => return Err(unknown_option(&arg)),
        }
    }
    Ok(Command::Run(opts))
}

/// Parse the arguments to `up-shell-init` (excluding the program name).
pub fn parse_up_init_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Command<CdCommand>, String> {
    let mut cd = CdCommand::Cd;
    for arg in args {
        match arg.to_str() {
            Some("--pushd") => cd = CdCommand::Pushd,
            Some("-h" | "--help") => return Ok(Command::Help),
            _ => return Err(unknown_option(&arg)),
        }
    }
    Ok(Command::Run(cd))
}

/// The path of the executable `name` that sits next to the running executable.
pub fn sibling_exe(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| {
        let argv0 = PathBuf::from(std::env::args_os().next().unwrap_or_default());
        if argv0.is_absolute() {
            argv0
        } else {
            let pwd = std::env::var_os("PWD").unwrap_or_else(|| ".".into());
            Path::new(&pwd).join(argv0)
        }
    });
    exe.with_file_name(name)
}

/// The shell function (and completion, for bash and zsh) that wraps `h`.
pub fn render_h_init(opts: &HInitOptions, h_exe: &str, code_root: &str, shell: Shell) -> String {
    let name = &opts.func_name;
    let cd = opts.cd.as_str();
    let mut out = if opts.git_opts.is_empty() {
        format!(
            "{name}() {{\n\
             \x20 _h_dir=$(command {h_exe} --resolve \"{code_root}\" \"$@\")\n\
             \x20 _h_ret=$?\n\
             \x20 [ \"$_h_dir\" != \"$PWD\" ] && {cd} \"$_h_dir\"\n\
             \x20 return $_h_ret\n\
             }}\n"
        )
    } else {
        let git_opts = &opts.git_opts;
        format!(
            "{name}() {{\n\
             \x20 _h_term=\"$1\"\n\
             \x20 shift\n\
             \x20 _h_dir=$(command {h_exe} --resolve \"{code_root}\" \"$_h_term\" {git_opts} \"$@\")\n\
             \x20 _h_ret=$?\n\
             \x20 [ \"$_h_dir\" != \"$PWD\" ] && {cd} \"$_h_dir\"\n\
             \x20 return $_h_ret\n\
             }}\n"
        )
    };

    // Only emit completion code for the detected shell: bash fails to parse zsh glob qualifiers
    // like `*(N/:t)` even inside an untaken branch of an `if`.
    match shell {
        Shell::Zsh => out.push_str(&format!(
            "_{name}_complete() {{\n\
             \x20 local code_root='{code_root}'\n\
             \x20 local -a projects\n\
             \x20 [[ -d \"$code_root\" ]] || return\n\
             \x20 projects=(\n\
             \x20   \"$code_root\"/*(N/:t)\n\
             \x20   \"$code_root\"/*/*(N/:t)\n\
             \x20   \"$code_root\"/*/*/*(N/:t)\n\
             \x20 )\n\
             \x20 projects=(\"${{(u)projects[@]}}\")\n\
             \x20 compadd -a projects\n\
             }}\n\
             compdef _{name}_complete {name}\n"
        )),
        Shell::Bash => out.push_str(&format!(
            "_{name}_complete() {{\n\
             \x20 local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n\
             \x20 local code_root='{code_root}'\n\
             \x20 COMPREPLY=()\n\
             \x20 [[ -d \"$code_root\" ]] || return\n\
             \x20 local dirs\n\
             \x20 dirs=$(find \"$code_root\" -mindepth 1 -maxdepth 3 -type d -not -name '.*' \
             2>/dev/null | sed 's|.*/||' | sort -u)\n\
             \x20 COMPREPLY=($(compgen -W \"$dirs\" -- \"$cur\"))\n\
             }}\n\
             complete -F _{name}_complete {name}\n"
        )),
        Shell::Unknown => {}
    }
    out
}

/// The shell function that wraps `up`.
pub fn render_up_init(cd: CdCommand, up_exe: &str) -> String {
    let cd = cd.as_str();
    format!(
        "up() {{\n\
         \x20 _up_dir=$(command {up_exe} \"$@\")\n\
         \x20 if [ $? = 0 ]; then\n\
         \x20   [ \"$_up_dir\" != \"$PWD\" ] && {cd} \"$_up_dir\"\n\
         \x20 fi\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_h_init_defaults() {
        assert_eq!(parse_h_init_args(args(&[])), Ok(Command::Run(HInitOptions::default())));
    }

    #[test]
    fn parses_h_init_options() {
        let parsed = parse_h_init_args(args(&[
            "--pushd",
            "--name",
            "j",
            "--git-opts",
            "--depth 1",
            "~/code",
        ]));
        assert_eq!(
            parsed,
            Ok(Command::Run(HInitOptions {
                func_name: "j".into(),
                cd: CdCommand::Pushd,
                git_opts: "--depth 1".into(),
                code_root: Some("~/code".into()),
            }))
        );
    }

    #[test]
    fn last_code_root_wins() {
        let Ok(Command::Run(opts)) = parse_h_init_args(args(&["a", "b"])) else { panic!() };
        assert_eq!(opts.code_root, Some("b".into()));
    }

    #[test]
    fn parses_h_init_help() {
        assert_eq!(parse_h_init_args(args(&["-h"])), Ok(Command::Help));
        assert_eq!(parse_h_init_args(args(&["--pushd", "--help", "--bogus"])), Ok(Command::Help));
    }

    #[test]
    fn rejects_unknown_h_init_options() {
        assert_eq!(parse_h_init_args(args(&["--bogus"])), Err("Unknown option: --bogus".into()));
        assert_eq!(parse_h_init_args(args(&["-"])), Err("Unknown option: -".into()));
        // Options missing their value are unknown.
        assert_eq!(parse_h_init_args(args(&["--name"])), Err("Unknown option: --name".into()));
        assert_eq!(
            parse_h_init_args(args(&["--git-opts"])),
            Err("Unknown option: --git-opts".into())
        );
    }

    #[test]
    fn parses_up_init_args() {
        assert_eq!(parse_up_init_args(args(&[])), Ok(Command::Run(CdCommand::Cd)));
        assert_eq!(parse_up_init_args(args(&["--pushd"])), Ok(Command::Run(CdCommand::Pushd)));
        assert_eq!(parse_up_init_args(args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse_up_init_args(args(&["x"])), Err("Unknown option: x".into()));
    }

    #[test]
    fn detects_shell_from_comm() {
        assert_eq!(Shell::from_comm("bash\n"), Shell::Bash);
        assert_eq!(Shell::from_comm("zsh\n"), Shell::Zsh);
        assert_eq!(Shell::from_comm("fish\n"), Shell::Unknown);
        assert_eq!(Shell::from_comm(""), Shell::Unknown);
    }

    #[test]
    fn sibling_exe_replaces_file_name() {
        let up = sibling_exe("up");
        assert_eq!(up.file_name(), Some(OsStr::new("up")));
        assert_eq!(up.parent(), std::env::current_exe().unwrap().parent());
    }

    #[test]
    fn renders_h_function() {
        let out = render_h_init(&HInitOptions::default(), "/bin/h", "/code", Shell::Unknown);
        assert_eq!(
            out,
            r#"h() {
  _h_dir=$(command /bin/h --resolve "/code" "$@")
  _h_ret=$?
  [ "$_h_dir" != "$PWD" ] && cd "$_h_dir"
  return $_h_ret
}
"#
        );
    }

    #[test]
    fn renders_h_function_with_git_opts() {
        let opts = HInitOptions {
            func_name: "j".into(),
            cd: CdCommand::Pushd,
            git_opts: "--depth 1".into(),
            code_root: None,
        };
        let out = render_h_init(&opts, "/bin/h", "/code", Shell::Unknown);
        assert_eq!(
            out,
            r#"j() {
  _h_term="$1"
  shift
  _h_dir=$(command /bin/h --resolve "/code" "$_h_term" --depth 1 "$@")
  _h_ret=$?
  [ "$_h_dir" != "$PWD" ] && pushd "$_h_dir"
  return $_h_ret
}
"#
        );
    }

    #[test]
    fn renders_bash_completion() {
        let out = render_h_init(&HInitOptions::default(), "/bin/h", "/code", Shell::Bash);
        let completion = out.split_once("}\n").unwrap().1;
        assert_eq!(
            completion,
            r#"_h_complete() {
  local cur="${COMP_WORDS[COMP_CWORD]}"
  local code_root='/code'
  COMPREPLY=()
  [[ -d "$code_root" ]] || return
  local dirs
  dirs=$(find "$code_root" -mindepth 1 -maxdepth 3 -type d -not -name '.*' 2>/dev/null | sed 's|.*/||' | sort -u)
  COMPREPLY=($(compgen -W "$dirs" -- "$cur"))
}
complete -F _h_complete h
"#
        );
    }

    #[test]
    fn renders_zsh_completion() {
        let opts = HInitOptions { func_name: "j".into(), ..Default::default() };
        let out = render_h_init(&opts, "/bin/h", "/code", Shell::Zsh);
        let completion = out.split_once("}\n").unwrap().1;
        assert_eq!(
            completion,
            r#"_j_complete() {
  local code_root='/code'
  local -a projects
  [[ -d "$code_root" ]] || return
  projects=(
    "$code_root"/*(N/:t)
    "$code_root"/*/*(N/:t)
    "$code_root"/*/*/*(N/:t)
  )
  projects=("${(u)projects[@]}")
  compadd -a projects
}
compdef _j_complete j
"#
        );
    }

    #[test]
    fn renders_up_function() {
        assert_eq!(
            render_up_init(CdCommand::Pushd, "/bin/up"),
            r#"up() {
  _up_dir=$(command /bin/up "$@")
  if [ $? = 0 ]; then
    [ "$_up_dir" != "$PWD" ] && pushd "$_up_dir"
  fi
}
"#
        );
    }
}
