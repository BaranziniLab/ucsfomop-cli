#!/usr/bin/env bash
# Rebuild dist/ucsfomop-bundle from the current source.
# Run this after any code change; commit the resulting dist/ directory.
set -euo pipefail

BREW=/opt/homebrew
BUNDLE=dist/ucsfomop-bundle
LIBDIR=$BUNDLE/lib/ucsfomop

echo "Building release binary..."
cargo build --release

echo "Assembling bundle..."
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/bin" "$LIBDIR"

cp target/release/ucsfomop "$BUNDLE/bin/ucsfomop"
cp "$BREW/opt/unixodbc/lib/libodbc.2.dylib"      "$LIBDIR/"
cp "$BREW/opt/unixodbc/lib/libodbcinst.2.dylib"  "$LIBDIR/"
cp "$BREW/lib/libtdsodbc.so"                     "$LIBDIR/"
cp "$BREW/opt/libtool/lib/libltdl.7.dylib"       "$LIBDIR/"
cp "$BREW/opt/openssl@3/lib/libssl.3.dylib"      "$LIBDIR/"
cp "$BREW/opt/openssl@3/lib/libcrypto.3.dylib"   "$LIBDIR/"
chmod -R u+w "$BUNDLE"

echo "Patching rpath references..."

install_name_tool \
  -change "$BREW/opt/unixodbc/lib/libodbc.2.dylib" @rpath/libodbc.2.dylib \
  -add_rpath @executable_path/../lib/ucsfomop \
  "$BUNDLE/bin/ucsfomop"

install_name_tool \
  -id @rpath/libodbc.2.dylib \
  -change "$BREW/opt/libtool/lib/libltdl.7.dylib" @loader_path/libltdl.7.dylib \
  "$LIBDIR/libodbc.2.dylib"

install_name_tool \
  -id @rpath/libodbcinst.2.dylib \
  -change "$BREW/opt/unixodbc/lib/libodbcinst.2.dylib" @rpath/libodbcinst.2.dylib \
  -change "$BREW/opt/libtool/lib/libltdl.7.dylib" @loader_path/libltdl.7.dylib \
  "$LIBDIR/libodbcinst.2.dylib"

install_name_tool \
  -id @loader_path/libtdsodbc.so \
  -change "$BREW/opt/unixodbc/lib/libodbc.2.dylib"     @loader_path/libodbc.2.dylib \
  -change "$BREW/opt/unixodbc/lib/libodbcinst.2.dylib" @loader_path/libodbcinst.2.dylib \
  -change "$BREW/opt/openssl@3/lib/libssl.3.dylib"     @loader_path/libssl.3.dylib \
  -change "$BREW/opt/openssl@3/lib/libcrypto.3.dylib"  @loader_path/libcrypto.3.dylib \
  "$LIBDIR/libtdsodbc.so"

install_name_tool -id @loader_path/libltdl.7.dylib "$LIBDIR/libltdl.7.dylib"

CRYPTO_REF=$(otool -L "$LIBDIR/libssl.3.dylib" | grep libcrypto | awk '{print $1}')
install_name_tool \
  -id @loader_path/libssl.3.dylib \
  -change "$CRYPTO_REF" @loader_path/libcrypto.3.dylib \
  "$LIBDIR/libssl.3.dylib"

install_name_tool -id @loader_path/libcrypto.3.dylib "$LIBDIR/libcrypto.3.dylib"

echo "Re-signing (ad-hoc)..."
for f in "$BUNDLE/bin/ucsfomop" "$LIBDIR"/*.dylib "$LIBDIR"/*.so; do
  codesign --force -s - "$f" 2>/dev/null
done

cp dist/ucsfomop-bundle/install.sh "$BUNDLE/install.sh" 2>/dev/null || true
chmod +x "$BUNDLE/install.sh"

echo ""
echo "Bundle ready at $BUNDLE/"
echo "Total size: $(du -sh $BUNDLE | cut -f1)"
