# optional file for user for recipes that don't belong
# in version control
import? 'local.justfile'

venv_path := absolute_path(".venv/bin")

export CARGO_TARGET_DIR := "target/just"
export PATH := venv_path + ":" + env('PATH')
alias y := why

[private]
@default:
  just --list --justfile {{justfile()}}

# setup development environment
setup:
  pre-commit install --install-hooks -t pre-commit -t commit-msg

# setup the venv if necessary
@venv python="python3":
  [ -d .venv ] || {{python}} -m venv .venv

# delete the venv and force pyo3 to re-configure accordingly
rm-venv:
  rm -rf .venv
  cargo clean --package pyo3-build-config
  cargo clean --package pyo3-build-config --target-dir target

# Build the project using cargo
[no-exit-message]
build: venv
  cargo build

# Test the project using cargo
[no-exit-message]
test *args: venv
  {{lib_path_var}}={{lib_path_var}}{{separator}}`{{lib_path_getter}}` \
  cargo nextest run {{args}}

# Run cargo check on the project
check *args: venv
  cargo check {{args}}

# Run clippy on the project
[no-exit-message]
clippy *args: venv
  cargo clippy {{args}}

# Build documentation using rustdoc
[no-exit-message]
doc *args: venv
  cargo doc --all-features {{args}}

# Run anything inside the venv
[no-exit-message]
exec program *args: venv
  {{lib_path_var}}={{lib_path_var}}{{separator}}`{{lib_path_getter}}` \
  {{program}} {{args}}

# Show graph for why a specific dependency was included (uses cargo tree)
[no-exit-message]
why package:
  cargo tree --target all --invert --package {{package}}

lib_path_getter := "python -c \"import sysconfig; print(sysconfig.get_config_var('LIBDIR'))\""
lib_path_var := if os() == "windows" {
  "PATH"
} else if os() == "macos" {
  "DYLD_LIBRARY_PATH"
} else if os() == "linux" {
  "LD_LIBRARY_PATH"
} else {
  error("unsupported os: add your os's dynamic lib path to the justfile")
}
separator := if os() == "windows" {
  ";"
} else {
  ":"
}
