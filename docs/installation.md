# Installation

Install Rho from this repository with Cargo:

```bash
cargo install --path .
```

Then run Rho directly:

```bash
rho
```

If Cargo's bin directory is not on your `PATH`, add it before running the [interactive TUI](/interactive-tui) or [automation commands](/automation-cli):

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Next, configure [authentication and models](/authentication-and-models).
