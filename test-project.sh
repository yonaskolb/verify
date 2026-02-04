#!/bin/bash
set -e

cargo build --quiet
./target/debug/verify --config test-project/verify.yaml "$@"
