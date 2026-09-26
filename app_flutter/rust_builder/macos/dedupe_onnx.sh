#!/bin/sh
# ONNX Runtime's static archive carries the onnx protobuf objects twice. A
# plain link pulls one and never looks at the other, but the podspec
# -force_loads the whole bridge archive (Dart finds the FFI symbols at run
# time, so nothing references them at link time) and the copies collide as
# duplicate symbols. Keep one of each.
set -e
LIB="$1"
T="$2/onnx_dedupe"
rm -rf "$T" && mkdir -p "$T"

# cargokit lipo's the archive even for one arch; ar only edits a thin one.
if lipo -info "$LIB" | grep "fat file" > /dev/null; then
  lipo -thin arm64 "$LIB" -output "$T/thin.a"
  mv "$T/thin.a" "$LIB"
fi

for m in onnx-ml.pb.cc.o onnx-operators-ml.pb.cc.o onnx-data.pb.cc.o; do
  [ "$(ar -t "$LIB" | grep -cx "$m")" -gt 1 ] || continue
  (cd "$T" && ar -x "$LIB" "$m")
  while ar -t "$LIB" | grep -x "$m" > /dev/null; do ar -d "$LIB" "$m"; done
  ar -q "$LIB" "$T/$m"
done
ranlib "$LIB"
