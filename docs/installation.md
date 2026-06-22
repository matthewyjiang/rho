# Installation

Install Rho from crates.io with Cargo:

```bash
cargo install rho-coding-agent
```

This installs the `rho` binary. Run it directly:

```bash
rho
```

If Cargo's bin directory is not on your `PATH`, add it before running the [interactive TUI](/interactive-tui) or [automation commands](/automation-cli):

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

Next, configure [authentication and models](/authentication-and-models).
