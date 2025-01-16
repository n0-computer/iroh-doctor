<h1 align="center"><a style="color:rgb(124 124 255)" href="https://iroh.computer">iroh-doctor</a></h1>

<h3 align="center">
Test your network.
</h3>

[![Documentation](https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square)](https://docs.rs/iroh-doctor/)
[![Crates.io](https://img.shields.io/crates/v/iroh.svg?style=flat-square)](https://crates.io/crates/iroh-doctor)
[![downloads](https://img.shields.io/crates/d/iroh.svg?style=flat-square)](https://crates.io/crates/iroh-doctor)
[![Chat](https://img.shields.io/discord/1161119546170687619?logo=discord&style=flat-square)](https://discord.com/invite/DpmJgtU7cW)
[![Youtube](https://img.shields.io/badge/YouTube-red?logo=youtube&logoColor=white&style=flat-square)](https://www.youtube.com/@n0computer)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg?style=flat-square)](LICENSE-APACHE)

<div align="center">
  <h3>
    <a href="https://iroh.computer/docs">
      Docs Site
    </a>
    <span> | </span>
    <a href="https://docs.rs/iroh">
      Rust Docs
    </a>
    <span> | </span>
    <a href="https://github.com/n0-computer/iroh/releases">
      Releases
    </a>
  </h3>
</div>
<br/>

## Overview

iroh-doctor is a tool used for diagnosing network issues with [iroh](https://github.com/n0-computer/iroh).

## Getting Started

Run `cargo install iroh-doctor` or build from source.

```shell
Usage: iroh-doctor [OPTIONS] <COMMAND>

Commands:
  report          Report on the current network environment, using either an explicitly provided stun host or the settings from the config file
  accept          Wait for incoming requests from iroh doctor connect
  connect         Connect to an iroh doctor accept node
  port-map-probe  Probe the port mapping protocols
  port-map        Attempt to get a port mapping to the given local port
  relay-urls      Get the latencies of the different relay url
  plot            Plot metric counters
  help            Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>
          Path to the configuration file, see https://iroh.computer/docs/reference/config

      --metrics-addr <METRICS_ADDR>
          Address to serve metrics on. Disabled by default

      --metrics-dump-path <METRICS_DUMP_PATH>
          Write metrics in CSV format at 100ms intervals. Disabled by default

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## License

Copyright 2024 N0, INC.

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
