#!/bin/bash

set -e

echo "🚀 Building React Native Rust HTTP Server with fixes..."

# Parse CLI flags
AUTO_CONFIRM=false
while getopts "y" opt; do
  case $opt in
    y)
      AUTO_CONFIRM=true
      echo "🤖 Auto-confirm enabled (-y)"
      ;;
    \?)
      echo "Invalid option: -$OPTARG" >&2
      exit 1
      ;;
  esac
done

echo "🧹 Cleaning previous builds..."
cargo clean

# 1. Generate bindings and build library
echo "🔧 Building Rust library..."
cargo build --release

if [ $? -ne 0 ]; then
    echo "❌ Failed to build Rust library"
    exit 1
fi

echo "✅ Rust library built successfully"

# 2. Build C test program
echo "🧪 Building C test program..."

OS_NAME=$(uname -s)
LIB_EXT=""
LINK_FLAGS=""

case "$OS_NAME" in
    Darwin)
        LIB_EXT="dylib"
        LIB_NAME="target/release/librn_http_server.dylib"
        LINK_FLAGS="-framework Security -framework Foundation"
        ;;
    Linux)
        LIB_EXT="so"
        LIB_NAME="target/release/librn_http_server.so"
        LINK_FLAGS="-lpthread -ldl"
        ;;
    *)
        echo "❌ Unsupported OS: $OS_NAME"
        exit 1
        ;;
esac

if [ ! -f "$LIB_NAME" ]; then
    echo "❌ Library not found: $LIB_NAME"
    exit 1
fi

if [ -f "examples/simple_test.c" ]; then
    echo "Compiling C test program..."
    
    gcc -std=c99 -I./include examples/simple_test.c \
        -Ltarget/release -lrn_http_server \
        $LINK_FLAGS \
        -o target/release/simple_test
    
    if [ $? -eq 0 ]; then
        echo "✅ C test program built: target/release/simple_test"
        echo ""
        echo "To run the test program:"
        echo "  cd target/release"
        echo "  ./simple_test"
        echo ""
        echo "Then in another terminal, test with curl:"
        echo "  curl http://localhost:8080/"
        echo "  curl -X POST http://localhost:8080/api -d '{\"test\":\"data\"}' -H 'Content-Type: application/json'"
    else
        echo "⚠️ Failed to build C test program, but Rust library was built successfully"
    fi
else
    echo "⚠️ C test program not found at examples/simple_test.c"
fi

# 3. Build iOS libraries (optional)
if [ "$AUTO_CONFIRM" = true ]; then
    REPLY="y"
else
    read -p "Do you want to build iOS libraries? (y/n): " -n 1 -r
    echo
fi
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo "📱 Building iOS libraries..."
    mkdir -p target/xcframework
    
    IOS_TARGETS=("x86_64-apple-ios" "aarch64-apple-ios-sim" "aarch64-apple-ios" "x86_64-apple-ios-macabi" "aarch64-apple-ios-macabi")
    
    for target in "${IOS_TARGETS[@]}"; do
        echo "  Building for $target..."
        cargo build --target "$target" --release
        
        if [ $? -ne 0 ]; then
            echo "❌ Failed to build for $target"
            exit 1
        fi
    done
    
    # Build dynamic frameworks (.dylib) instead of static archives (.a) to avoid symbol conflicts with other Rust modules.
    
    echo "  Creating universal simulator dynamic library..."
    mkdir -p target/universal/ios-sim
    xcrun lipo -create \
        target/x86_64-apple-ios/release/librn_http_server.dylib \
        target/aarch64-apple-ios-sim/release/librn_http_server.dylib \
        -output target/universal/ios-sim/librn_http_server.dylib
    
    echo "  Preparing device dynamic library..."
    mkdir -p target/universal/ios-device
    cp target/aarch64-apple-ios/release/librn_http_server.dylib \
       target/universal/ios-device/librn_http_server.dylib
       
    echo "  Creating Mac Catalyst dynamic library..."
    mkdir -p target/universal/ios-macabi
    xcrun lipo -create \
        target/x86_64-apple-ios-macabi/release/librn_http_server.dylib \
        target/aarch64-apple-ios-macabi/release/librn_http_server.dylib \
        -output target/universal/ios-macabi/librn_http_server.dylib
    
    echo "  Setting install names..."
    install_name_tool -id @rpath/RNHttpServer.framework/RNHttpServer \
        target/universal/ios-sim/librn_http_server.dylib
    install_name_tool -id @rpath/RNHttpServer.framework/RNHttpServer \
        target/universal/ios-device/librn_http_server.dylib
    install_name_tool -id @rpath/RNHttpServer.framework/Versions/A/RNHttpServer \
        target/universal/ios-macabi/librn_http_server.dylib
        
    echo "  Creating Framework structure..."
    FRAMEWORK_SIM="target/universal/ios-sim/RNHttpServer.framework"
    FRAMEWORK_DEVICE="target/universal/ios-device/RNHttpServer.framework"
    FRAMEWORK_MACABI="target/universal/ios-macabi/RNHttpServer.framework"
    
    rm -rf "$FRAMEWORK_SIM" "$FRAMEWORK_DEVICE" "$FRAMEWORK_MACABI"
    
    # Simulator Framework (iOS shallow bundle)
    mkdir -p "$FRAMEWORK_SIM/Headers"
    cp target/universal/ios-sim/librn_http_server.dylib "$FRAMEWORK_SIM/RNHttpServer"
    cp include/*.h "$FRAMEWORK_SIM/Headers/" 2>/dev/null || echo "Info: No headers found in include/"
    
    # Device Framework (iOS shallow bundle)
    mkdir -p "$FRAMEWORK_DEVICE/Headers"
    cp target/universal/ios-device/librn_http_server.dylib "$FRAMEWORK_DEVICE/RNHttpServer"
    cp include/*.h "$FRAMEWORK_DEVICE/Headers/" 2>/dev/null || echo "Info: No headers found in include/"
    
    # Mac Catalyst Framework (macOS deep bundle)
    mkdir -p "$FRAMEWORK_MACABI/Versions/A/Headers"
    mkdir -p "$FRAMEWORK_MACABI/Versions/A/Resources"
    cp target/universal/ios-macabi/librn_http_server.dylib "$FRAMEWORK_MACABI/Versions/A/RNHttpServer"
    cp include/*.h "$FRAMEWORK_MACABI/Versions/A/Headers/" 2>/dev/null || echo "Info: No headers found in include/"
    
    # Use relative symlinks executed from the target directory, otherwise xcodebuild ignores broken links.
    pushd "$FRAMEWORK_MACABI/Versions" > /dev/null
    ln -s "A" "Current"
    popd > /dev/null
    
    pushd "$FRAMEWORK_MACABI" > /dev/null
    ln -s "Versions/Current/Headers" "Headers"
    ln -s "Versions/Current/Resources" "Resources"
    ln -s "Versions/Current/RNHttpServer" "RNHttpServer"
    popd > /dev/null
    
    cat > "$FRAMEWORK_SIM/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>RNHttpServer</string>
    <key>CFBundleIdentifier</key>
    <string>com.iwater.RNHttpServer</string>
    <key>CFBundleName</key>
    <string>RNHttpServer</string>
    <key>CFBundlePackageType</key>
    <string>FMWK</string>
    <key>CFBundleVersion</key>
    <string>1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>MinimumOSVersion</key>
    <string>13.0</string>
</dict>
</plist>
PLIST
    cp "$FRAMEWORK_SIM/Info.plist" "$FRAMEWORK_DEVICE/Info.plist"
    # maccatalyst requires Info.plist in the root for XCFramework creation, AND in Resources/ for deep bundle
    cp "$FRAMEWORK_SIM/Info.plist" "$FRAMEWORK_MACABI/Versions/A/Resources/Info.plist"
    
    echo "  Creating XCFramework..."
    rm -rf target/xcframework/RNHttpServer.xcframework
    
    xcodebuild -create-xcframework \
        -framework "$FRAMEWORK_SIM" \
        -framework "$FRAMEWORK_DEVICE" \
        -framework "$FRAMEWORK_MACABI" \
        -output target/xcframework/RNHttpServer.xcframework
    
    if [ $? -eq 0 ]; then
        echo "✅ iOS XCFramework built successfully!"
        echo "  XCFramework: target/xcframework/RNHttpServer.xcframework"
        echo ""
        echo "  You can now integrate this XCFramework into your iOS project:"
        echo "  1. Drag RNHttpServer.xcframework to your Xcode project"
        echo "  2. In your target's 'Frameworks, Libraries, and Embedded Content', verify it's added"
        echo "  3. Import the header: #import <RNHttpServer/rn_http_server.h>"
    else
        echo "❌ Failed to create XCFramework"
        exit 1
    fi
fi


# 4. Build Android libraries (optional)
if [ "$AUTO_CONFIRM" = true ]; then
    REPLY="y"
else
    read -p "Do you want to build Android libraries? (y/n): " -n 1 -r
    echo
fi
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo "🤖 Building Android libraries..."
    
    if [ -z "$ANDROID_NDK_HOME" ]; then
        ANDROID_NDK_HOME="$HOME/Library/Android/sdk/ndk/27.1.12297006"
        if [ ! -d "$ANDROID_NDK_HOME" ]; then
            echo "❌ Android NDK not found at $ANDROID_NDK_HOME"
            echo "Please set ANDROID_NDK_HOME environment variable"
            exit 1
        fi
    fi
    
    echo "  Using NDK at: $ANDROID_NDK_HOME"
    
    ANDROID_TARGETS=(
        "aarch64-linux-android"
        "armv7-linux-androideabi" 
        "i686-linux-android"
        "x86_64-linux-android"
    )
    
    for target in "${ANDROID_TARGETS[@]}"; do
        echo "  Building for $target..."
        RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" cargo ndk --target "$target" --platform 21 build --release
        
        if [ $? -ne 0 ]; then
            echo "❌ Failed to build for $target"
            exit 1
        fi
    done
    
    mkdir -p target/universal/android/{arm64-v8a,armeabi-v7a,x86,x86_64}
    
    cp target/aarch64-linux-android/release/librn_http_server.so \
       target/universal/android/arm64-v8a/
    cp target/armv7-linux-androideabi/release/librn_http_server.so \
       target/universal/android/armeabi-v7a/
    cp target/i686-linux-android/release/librn_http_server.so \
       target/universal/android/x86/
    cp target/x86_64-linux-android/release/librn_http_server.so \
       target/universal/android/x86_64/
    
    echo "✅ Android libraries built successfully!"
    echo "  ARM64: target/universal/android/arm64-v8a/librn_http_server.so"
    echo "  ARMv7: target/universal/android/armeabi-v7a/librn_http_server.so"
    echo "  x86: target/universal/android/x86/librn_http_server.so"
    echo "  x86_64: target/universal/android/x86_64/librn_http_server.so"
fi

# 5. Run Rust tests
# Must use --test-threads=1 because server handles, callbacks, CORS config, and
# request/response maps are process-wide singletons.
echo "🧪 Running Rust tests..."
if cargo test -- --test-threads=1 --nocapture; then
    echo "✅ All tests passed!"
else
    echo "⚠️ Some tests failed, but build succeeded"
fi

echo ""
echo "🎉 Build completed successfully!"
echo ""
echo "📁 Output summary:"
echo "  - Rust library: target/release/librn_http_server.$LIB_EXT"
echo "  - Header file: include/rn_http_server.h"
if [ -f "target/release/simple_test" ]; then
    echo "  - C test program: target/release/simple_test"
fi
if [ -f "target/xcframework/RNHttpServer.xcframework/Info.plist" ]; then
    echo "  - iOS XCFramework: target/xcframework/RNHttpServer.xcframework"
fi
if [ -d "target/universal/android" ]; then
    echo "  - Android libraries: target/universal/android/"
fi