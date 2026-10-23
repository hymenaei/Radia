Radia is a Second Life viewer written in Rust. It redesigns viewer systems while preserving compatibility with Second Life protocols, content, and workflows.

The current milestone is the Classic compatibility baseline. Radia does not identify or offer selectable editions while that work is in progress.

## Build and run

Install the stable Rust toolchain through rustup. On Windows, install the Visual Studio C++ Build Tools and Windows SDK. See [tooling.md](docs/tooling.md) for platform setup and development commands.

From the workspace root, run:

```sh
cargo run
```

## Project docs

- [Vision](docs/vision.md)
- [Architecture](docs/architecture.md)
- [Legacy viewer reference guide](docs/reference.md)
- [Build, test, and profiling commands](docs/tooling.md)
