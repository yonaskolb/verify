#!/bin/bash
set -e

cargo build --quiet
./target/debug/vfy --config test-project/vfy.yaml "$@"
