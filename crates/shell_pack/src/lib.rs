pub mod elf_stub;
pub mod layout;
pub mod packer;
pub mod protect;

pub use elf_stub::{stub_bytes, target_machine, validate_stub, ElfMachine};
pub use layout::ScLayout;
pub use packer::Packer;
pub use protect::{is_protected, open, ProtectHeader, ProtectOptions};
