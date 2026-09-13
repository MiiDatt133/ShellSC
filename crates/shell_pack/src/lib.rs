pub mod elf_stub;
pub mod layout;
pub mod packer;
pub mod protect;
pub mod variant;

pub use elf_stub::{stub_bytes, target_machine, validate_stub, ElfMachine};
pub use layout::ScLayout;
pub use packer::Packer;
pub use protect::{derive_seeds, is_protected, open, DerivedSeeds, ProtectHeader, ProtectOptions};
pub use variant::{build_variant_stub, variant_seed, workspace_root};
