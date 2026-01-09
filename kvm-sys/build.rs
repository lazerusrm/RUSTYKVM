fn main() {
    // With dynamic loading via libloading, we don't need to link against libkvm
    // at compile time. The library will be loaded at runtime from:
    // - ./dl_lib/libkvm.so (relative to binary)
    // - /kvmapp/server/dl_lib/libkvm.so
    // - /tmp/server/dl_lib/libkvm.so
    //
    // If the library isn't found, video capture functions will return errors
    // but the server will still start and serve other functionality.

    println!("cargo:rerun-if-changed=build.rs");
}
