mod fs;
mod fw;
mod helpers;
mod igvm;
mod kernel;

use crate::{
    fs::FsConfig, fw::FirmwareConfig, helpers::HELPERS, igvm::IgvmConfig, kernel::KernelConfig,
};
use clap::Parser;
use serde::Deserialize;
use std::borrow::{Borrow, BorrowMut};
use std::env;
use std::error::Error;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

type BuildResult<T> = Result<T, Box<dyn Error>>;

/// A generic component that needs to be built
struct Component<S: AsRef<str>, B: Borrow<ComponentConfig>> {
    /// The name of the component
    name: S,
    /// The configuration to build the component
    config: B,
}

impl<S: AsRef<str>> Component<S, ComponentConfig> {
    fn new_default(name: S) -> Self {
        Self {
            name,
            config: ComponentConfig::default(),
        }
    }
}

impl<S: AsRef<str>, B: Borrow<ComponentConfig>> Component<S, B> {
    /// Create a new component with the given name an configuration.
    const fn new(name: S, config: B) -> Self {
        Self { name, config }
    }

    /// Build the component with the given user arguments and target.
    fn build(&self, args: &Args, target: BuildTarget) -> BuildResult<PathBuf> {
        println!("Building {}...", self.name.as_ref());
        self.config.borrow().build(args, self.name.as_ref(), target)
    }
}

/// Run a command and check its exit status
fn run_cmd_checked<C: BorrowMut<Command>>(mut cmd: C, args: &Args) -> BuildResult<()> {
    if args.verbose {
        println!("{:?}", cmd.borrow());
    }
    if cmd.borrow_mut().status()?.success() {
        return Ok(());
    }
    Err(std::io::Error::last_os_error().into())
}

/// Build targets for cargo
#[derive(Clone, Copy, Debug)]
enum BuildTarget {
    X8664UnknownNone,
    Host,
}

impl BuildTarget {
    const fn svsm_kernel() -> Self {
        Self::X8664UnknownNone
    }

    const fn svsm_user() -> Self {
        Self::X8664UnknownNone
    }
}

impl AsRef<str> for BuildTarget {
    fn as_ref(&self) -> &str {
        match self {
            Self::X8664UnknownNone => "x86_64-unknown-none",
            // We get this from build.rs
            Self::Host => env!("HOST_TARGET"),
        }
    }
}

/// Available methods to build a component
#[derive(Clone, Copy, Debug, Deserialize, Default)]
enum BuildType {
    #[default]
    #[serde(rename = "cargo")]
    Cargo,
    #[serde(rename = "make")]
    Makefile,
}

/// Binutils target used in objcopy
#[derive(Clone, Debug, Deserialize)]
struct Objcopy(String);

impl Default for Objcopy {
    fn default() -> Self {
        Self("elf64-x86-64".into())
    }
}

impl Objcopy {
    /// Call `objcopy` with the given input and output files
    fn copy(&self, src: &Path, dst: &Path, args: &Args) -> BuildResult<()> {
        let mut cmd = Command::new("objcopy");
        cmd.arg("-O")
            .arg(&self.0)
            .arg("--strip-unneeded")
            .arg(src)
            .arg(dst);
        run_cmd_checked(cmd, args)
    }
}

/// The recipe for a single kernel component (e.g. `tdx-stage1`,
/// `stage2` or `svsm`.
#[derive(Clone, Debug, Deserialize, Default)]
struct ComponentConfig {
    #[serde(rename = "type", default)]
    build_type: BuildType,
    output_file: Option<String>,
    manifest: Option<PathBuf>,
    #[serde(default)]
    features: Option<String>,
    #[serde(default)]
    binary: bool,
    #[serde(default)]
    objcopy: Objcopy,
    path: Option<PathBuf>,
}

impl ComponentConfig {
    /// Build this component with the specified target
    fn build(&self, args: &Args, pkg: &str, target: BuildTarget) -> BuildResult<PathBuf> {
        match self.build_type {
            BuildType::Cargo => self.cargo_build(args, pkg, target),
            BuildType::Makefile => self.makefile_build(args),
        }
    }

    /// Build this component as a cargo binary
    fn cargo_build(&self, args: &Args, pkg: &str, target: BuildTarget) -> BuildResult<PathBuf> {
        let mut cmd = Command::new("cargo");
        cmd.args([
            "build",
            if self.binary { "--bin" } else { "--package" },
            pkg,
            "--target",
            target.as_ref(),
        ]);
        if let Some(feat) = self.features.as_ref() {
            cmd.args(["--features", feat]);
        };
        if let Some(manifest) = self.manifest.as_ref() {
            cmd.args(["--manifest-path".as_ref(), manifest.as_os_str()]);
        }
        if args.release {
            cmd.arg("--release");
        }
        if args.offline {
            cmd.args(["--offline", "--locked"]);
        }
        if args.verbose {
            cmd.arg("-vv");
        }
        run_cmd_checked(cmd, args)?;

        // Get the path to the resulting binary
        Ok(PathBuf::from_iter([
            "target",
            target.as_ref(),
            if args.release { "release" } else { "debug" },
            pkg,
        ]))
    }

    /// Build this component as a Makefile binary.
    fn makefile_build(&self, args: &Args) -> BuildResult<PathBuf> {
        let Some(file) = self.output_file.as_ref() else {
            return Err("Cannot build makefile target without output_file".into());
        };
        let mut cmd = Command::new("make");
        cmd.arg(file);
        if args.release {
            cmd.arg("RELEASE=1");
        }
        if args.verbose {
            cmd.arg("V=2");
        }
        run_cmd_checked(cmd, args)?;
        Ok(PathBuf::from(file))
    }
}

/// A recipe corresponding to a full build.
#[derive(Clone, Debug, Deserialize)]
struct Recipe {
    /// SVSM kernel components
    kernel: KernelConfig,
    /// Guest firmware components
    #[serde(default)]
    firmware: FirmwareConfig,
    /// Guest filesystem components
    fs: FsConfig,
    /// IGVM configuration
    igvm: IgvmConfig,
}

impl Recipe {
    /// Builds the kernel components for this recipe. Returns a
    /// [`RecipePartsBuilder`] that can be used to keep track of
    /// built components for the recipe.
    fn build_kernel(&self, args: &Args) -> BuildResult<RecipePartsBuilder> {
        let mut parts = RecipePartsBuilder::new();
        for obj in self.kernel.build(args)? {
            match obj.file_name().and_then(|s| s.to_str()).unwrap_or_default() {
                "tdx-stage1" => parts.set_stage1(obj),
                "stage2" => parts.set_stage2(obj),
                "svsm" => parts.set_kernel(obj),
                n => eprintln!("WARN: kernel: ignoring unknown component: {n}"),
            }
        }
        Ok(parts)
    }

    /// Builds all the components for this recipe
    fn build(&self, args: &Args) -> BuildResult<()> {
        // Build kernel, guest firmware and guest filesystem
        let mut parts = self.build_kernel(args)?;
        if let Some(fw) = self.firmware.build(args)? {
            parts.set_fw(fw);
        }
        if let Some(fs) = self.fs.build(args)? {
            parts.set_fs(fs);
        }

        // Check that we have all pieces and build the IGVM file
        let parts = parts.build()?;
        self.igvm.build(args, &parts)?;
        Ok(())
    }
}

/// A helper structure used to keep track of all components built by
/// a recipe.
#[derive(Debug, Default, Clone)]
struct RecipePartsBuilder {
    stage1: Option<PathBuf>,
    stage2: Option<PathBuf>,
    kernel: Option<PathBuf>,
    firmware: Option<PathBuf>,
    fs: Option<PathBuf>,
}

impl RecipePartsBuilder {
    fn new() -> Self {
        Self::default()
    }

    fn set_stage1(&mut self, v: PathBuf) {
        self.stage1 = Some(v);
    }

    fn set_stage2(&mut self, v: PathBuf) {
        self.stage2 = Some(v);
    }

    fn set_kernel(&mut self, v: PathBuf) {
        self.kernel = Some(v)
    }

    fn set_fw(&mut self, v: PathBuf) {
        self.firmware = Some(v);
    }

    fn set_fs(&mut self, v: PathBuf) {
        self.fs = Some(v);
    }

    /// Returns a [`RecipeParts`] if all required components have
    /// been built.
    fn build(self) -> BuildResult<RecipeParts> {
        Ok(RecipeParts {
            stage1: self.stage1,
            stage2: self.stage2.ok_or("kernel: missing stage2")?,
            kernel: self.kernel.ok_or("kernel: missing main kernel")?,
            firmware: self.firmware,
            fs: self.fs,
        })
    }
}

/// Components built by a recipe. Used by IGVM tools to build the
/// final image.
#[derive(Clone, Debug)]
struct RecipeParts {
    stage1: Option<PathBuf>,
    stage2: PathBuf,
    kernel: PathBuf,
    firmware: Option<PathBuf>,
    fs: Option<PathBuf>,
}

#[derive(clap::Parser, Debug)]
#[clap(version, about = "SVSM build tool")]
struct Args {
    /// Perform a release build (default: false)
    #[clap(short, long, value_parser)]
    release: bool,
    /// Enable verbose output (default: false)
    #[clap(short, long, value_parser)]
    verbose: bool,
    /// Perform offline build (default: false)
    #[clap(short, long, value_parser)]
    offline: bool,
    /// Print each recipe before building (default: false)
    #[clap(short, long, value_parser)]
    print_config: bool,
    // Path to the JSON build recipe(s)
    #[clap(required(true))]
    recipes: Vec<PathBuf>,
}

fn main() -> Result<(), Box<dyn Error>> {
    // TODO: chekc current path

    let args = Args::parse();

    for filename in args.recipes.iter() {
        let f = File::open(filename)?;
        let recipe = serde_json::from_reader::<_, Recipe>(f)?;
        if args.print_config {
            println!("{}: {recipe:#?}", filename.display());
        }
        recipe.build(&args)?;
    }

    Ok(())
}
