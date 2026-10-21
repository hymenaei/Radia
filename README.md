Radia is a modern third party Second Life viewer written in Rust, focused on rethinking and improving the architecture, performance, usability, and maintainability of existing Second Life viewers.

The project aims to preserve compatibility with the Second Life platform while replacing legacy systems with a cleaner, more modular, and modern implementation. Areas of focus include networking, rendering, user interface, asset handling, and the overall viewer architecture.

### Build and run

Install the stable Rust toolchain through rustup. On Windows, install the Visual Studio C++ Build Tools as well, since some dependencies may require a native compiler and Windows SDK.

Then, from the workspace root, run:

```cmd
cargo run
```

Cargo will download the required dependencies, build Radia, and launch the application.
