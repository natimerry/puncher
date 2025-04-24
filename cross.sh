#!/bin/bash

export CC_mips_unknown_linux_musl="mips-linux-muslsf-gcc"
export CXX_mips_unknown_linux_musl="mips-linux-muslsf-g++"
export AR_mips_unknown_linux_musl="mips-linux-muslsf-ar"
export CARGO_TARGET_MIPS_UNKNOWN_LINUX_MUSL_LINKER="mips-linux-muslsf-gcc"

cargo +nightly build -Z build-std=std,panic_abort --target mips-unknown-linux-musl --release