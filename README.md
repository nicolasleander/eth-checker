# Run production build

make run-prod NUM_MNEMONICS=1000 MODE=local THREADS=16

# Run with predefined list

make run-prod-predefined MODE=local

# Run benchmarks

make benchmark

# Profile performance

make profile

# Optimize database

make optimize-db

# For benchmarking

cargo install hyperfine

# For profiling (on Ubuntu/Debian)

sudo apt install linux-tools-common linux-tools-generic

# For static linking

sudo apt install musl-tools
