blueprints = custom_target(
  'blueprints',
  input: files(
    'widgets/entry_dialog.blp',
    'widgets/new_project_dialog.blp',
    'widgets/review_page.blp',
    'widgets/task_picker.blp',
    'widgets/track_page.blp',
    'keyboard_shortcuts.blp',
    'window.blp',
  ),
  output: '.',
  command: [
    blueprint_compiler,
    'batch-compile',
    '@OUTPUT@',
    '@CURRENT_SOURCE_DIR@',
    '@INPUT@',
  ],
)

resources = gnome.compile_resources(
  'resources',
  'resources.gresource.xml',
  gresource_bundle: true,
  # Compiled .ui files live in the build dir, style.css in the source dir.
  source_dir: [meson.current_build_dir(), meson.current_source_dir()],
  install: true,
  install_dir: pkgdatadir,
  dependencies: blueprints,
)

# Build-time configuration is passed to rustc as environment variables and
# read in src/config.rs with option_env!(), so plain `cargo check` (and
# rust-analyzer) keep working without a generated source file.
cargo_env = [
  'CARGO_HOME=' + meson.project_build_root() / 'cargo-home',
  'TT_APP_ID=' + application_id,
  'TT_PROFILE=' + profile,
  'TT_VERSION=' + version + version_suffix,
  'TT_GETTEXT_PACKAGE=' + gettext_package,
  'TT_LOCALEDIR=' + localedir,
  'TT_PKGDATADIR=' + pkgdatadir,
]

cargo_options = [
  '--manifest-path', meson.project_source_root() / 'Cargo.toml',
  '--target-dir', meson.project_build_root() / 'src',
]

if get_option('profile') == 'default'
  cargo_options += ['--release']
  rust_target = 'release'
  message('Building in release mode')
else
  rust_target = 'debug'
  message('Building in debug mode')
endif

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: bindir,
  depends: resources,
  command: [
    'env',
    cargo_env,
    cargo,
    'build',
    cargo_options,
    '&&',
    'cp',
    'src' / rust_target / meson.project_name(),
    '@OUTPUT@',
  ],
)

test(
  'cargo-test',
  cargo,
  args: ['test', cargo_options],
  env: cargo_env,
  timeout: 600,
)
