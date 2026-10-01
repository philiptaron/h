//! The shell function printed by `h-shell-init`.

use std::ffi::OsString;

use h_git::shell::{CdCommand, Command, Shell, unknown_option};

pub const H_USAGE: &str =
    "Usage: eval \"$(h-shell-init [--pushd] [--name NAME] [--git-opts \"OPTIONS\"] [code-root])\"";

/// Environment variable holding the default code root.
pub const CODE_ROOT_ENV: &str = "H_CODE_ROOT";
/// Code root used when neither an argument nor [`CODE_ROOT_ENV`] gives one.
pub const DEFAULT_CODE_ROOT: &str = "~/src";

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
}
