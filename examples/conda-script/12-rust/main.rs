// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "rustc -o ${CACHE}/main ${SCRIPT} -C linker=gcc && ${CACHE}/main"
//
// [dependencies]
// rust = "*"
// gcc = "*"
// zlib = "*"
// /// end-conda-script
use std::ffi::c_ulong;

#[link(name = "z")]
extern "C" {
    fn crc32(crc: c_ulong, buf: *const u8, len: u32) -> c_ulong;
}

fn main() {
    let message = b"conda-script";
    let checksum = unsafe { crc32(0, message.as_ptr(), message.len() as u32) };
    println!("crc32(\"conda-script\") = 0x{checksum:08x}");
}
