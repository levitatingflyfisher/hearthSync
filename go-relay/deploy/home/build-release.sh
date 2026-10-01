#!/usr/bin/env bash
# Build the Go relay as static, single-file binaries for the machines a household
# is likely to have spare: a 64-bit PC (amd64), a 64-bit Raspberry Pi 3/4/5
# (arm64) and a Raspberry Pi on a 32-bit OS (armv7). Pure Go, no cgo, so no
# cross-compiler is needed.
#
#   go-relay/deploy/home/build-release.sh [OUT_DIR]     (default: go-relay/dist)
#
# Writes hearth-relay-go-<version>-linux-<arch> and SHA256SUMS. The flags make
# the build reproducible: the same source and Go version give the same bytes.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
src=$(cd "$here/../.." && pwd)
out=${1:-$src/dist}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$src"
version=$(sed -n 's/^const version = "\(.*\)"$/\1/p' cmd/hearth-relay-go/main.go)
export CGO_ENABLED=0 GOTOOLCHAIN=local GOFLAGS=-mod=readonly
for target in amd64 arm64 arm:7; do
  arch=${target%%:*}
  name=hearth-relay-go-$version-linux-$arch
  if [ "$arch" = arm ]; then
    export GOARM=${target##*:}
    name=$name"v$GOARM"
  else
    unset GOARM
  fi
  GOOS=linux GOARCH=$arch go build -trimpath -buildvcs=false -ldflags='-s -w -buildid=' \
    -o "$out/$name" ./cmd/hearth-relay-go
  echo "built $out/$name"
done
(cd "$out" && sha256sum hearth-relay-go-"$version"-linux-* > SHA256SUMS && cat SHA256SUMS)
