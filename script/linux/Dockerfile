# Trek's Linux build and test environment: Ubuntu 22.04, the oldest glibc (2.35) Trek's Linux
# builds support, with what `cargo build -p trek-app` links against and what runs it with no
# display of its own (Xvfb, Weston's headless backend, Mesa's software Vulkan). CI's ubuntu-22.04
# runners install the same packages (script/linux/packages.sh); this image is for running the
# Linux build on a Mac or any other machine with Docker:
#
#   docker build -t trek-linux script/linux
#   docker run --rm -it -v "$PWD":/src -v trek-linux-target:/target trek-linux
#
# The architecture is the host's: arm64 on Apple silicon, x86_64 on a PC.
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
COPY packages.sh /tmp/packages.sh
RUN sh /tmp/packages.sh --with-run-tools --with-packaging && rm -rf /var/lib/apt/lists/*

# The toolchain rust-toolchain.toml names (rustup reads it from the source tree; installing it
# here keeps it in the image rather than downloading it on every run).
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.99.0 \
    --component clippy --component rustfmt \
 && chmod -R a+w /opt/rustup /opt/cargo

ENV CARGO_TARGET_DIR=/target CARGO_INCREMENTAL=0
WORKDIR /src
