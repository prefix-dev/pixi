// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "zig run ${SCRIPT} $(pkg-config --libs zlib)"
//
// [dependencies]
// zig = "*"
// zlib = "*"
// pkg-config = "*"
// /// end-conda-script

const std = @import("std");
const Io = std.Io;

extern fn crc32(crc: c_ulong, buf: [*]const u8, len: c_uint) c_ulong;

pub fn main(init: std.process.Init) !void {
    const data = "hello world";
    const sum = crc32(0, data, data.len);
    var buffer: [64]u8 = undefined;
    var file_writer: Io.File.Writer = .init(.stdout(), init.io, &buffer);
    const out = &file_writer.interface;
    try out.print("crc32(hello world) = {d}\n", .{sum});
    try out.flush();
}
