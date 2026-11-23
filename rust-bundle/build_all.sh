#!/bin/bash
# build_all_fixed.sh

set -e

echo "🚀 Building React Native Rust HTTP Server with fixes..."

# 解析命令行参数
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

# 清理
echo "🧹 Cleaning previous builds..."
cargo clean

# 1. 生成绑定并构建库
echo "🔧 Building Rust library..."
cargo build --release

if [ $? -ne 0 ]; then
    echo "❌ Failed to build Rust library"
    exit 1
fi

echo "✅ Rust library built successfully"

# 2. 构建 C 测试程序
echo "🧪 Building C test program..."

# 检测操作系统
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

# 构建 C 程序
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

# 3. 构建 iOS 库（可选）
if [ "$AUTO_CONFIRM" = true ]; then
    REPLY="y"
else
    read -p "Do you want to build iOS libraries? (y/n): " -n 1 -r
    echo
fi
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo "📱 Building iOS libraries..."
    mkdir -p target/xcframework
    
    # 构建各个架构
    IOS_TARGETS=("x86_64-apple-ios" "aarch64-apple-ios-sim" "aarch64-apple-ios" "x86_64-apple-ios-macabi" "aarch64-apple-ios-macabi")
    
    for target in "${IOS_TARGETS[@]}"; do
        echo "  Building for $target..."
        cargo build --target "$target" --release
        
        if [ $? -ne 0 ]; then
            echo "❌ Failed to build for $target"
            exit 1
        fi
    done
    
    # ===== 动态库构建 (Dynamic Framework) =====
    # 使用 dylib 代替 .a 以解决与其他 Rust 模块的符号冲突
    
    # 创建模拟器通用动态库
    echo "  Creating universal simulator dynamic library..."
    mkdir -p target/universal/ios-sim
    xcrun lipo -create \
        target/x86_64-apple-ios/release/librn_http_server.dylib \
        target/aarch64-apple-ios-sim/release/librn_http_server.dylib \
        -output target/universal/ios-sim/librn_http_server.dylib
    
    # 创建设备动态库目录
    echo "  Preparing device dynamic library..."
    mkdir -p target/universal/ios-device
    cp target/aarch64-apple-ios/release/librn_http_server.dylib \
       target/universal/ios-device/librn_http_server.dylib
       
    # 创建 Mac Catalyst 动态库
    echo "  Creating Mac Catalyst dynamic library..."
    mkdir -p target/universal/ios-macabi
    xcrun lipo -create \
        target/x86_64-apple-ios-macabi/release/librn_http_server.dylib \
        target/aarch64-apple-ios-macabi/release/librn_http_server.dylib \
        -output target/universal/ios-macabi/librn_http_server.dylib
    
    # 设置正确的 install_name (使用 @rpath)
    echo "  Setting install names..."
    install_name_tool -id @rpath/RNHttpServer.framework/RNHttpServer \
        target/universal/ios-sim/librn_http_server.dylib
    install_name_tool -id @rpath/RNHttpServer.framework/RNHttpServer \
        target/universal/ios-device/librn_http_server.dylib
    install_name_tool -id @rpath/RNHttpServer.framework/Versions/A/RNHttpServer \
        target/universal/ios-macabi/librn_http_server.dylib
        
    # 创建 Framework 结构
    echo "  Creating Framework structure..."
    FRAMEWORK_SIM="target/universal/ios-sim/RNHttpServer.framework"
    FRAMEWORK_DEVICE="target/universal/ios-device/RNHttpServer.framework"
    FRAMEWORK_MACABI="target/universal/ios-macabi/RNHttpServer.framework"
    
    # 清理旧的 Framework
    rm -rf "$FRAMEWORK_SIM" "$FRAMEWORK_DEVICE" "$FRAMEWORK_MACABI"
    
    # 创建模拟器 Framework (iOS shallow bundle)
    mkdir -p "$FRAMEWORK_SIM/Headers"
    cp target/universal/ios-sim/librn_http_server.dylib "$FRAMEWORK_SIM/RNHttpServer"
    cp include/*.h "$FRAMEWORK_SIM/Headers/" 2>/dev/null || echo "Info: No headers found in include/"
    
    # 创建设备 Framework (iOS shallow bundle)
    mkdir -p "$FRAMEWORK_DEVICE/Headers"
    cp target/universal/ios-device/librn_http_server.dylib "$FRAMEWORK_DEVICE/RNHttpServer"
    cp include/*.h "$FRAMEWORK_DEVICE/Headers/" 2>/dev/null || echo "Info: No headers found in include/"
    
    # 创建 Mac Catalyst Framework (macOS deep bundle)
    mkdir -p "$FRAMEWORK_MACABI/Versions/A/Headers"
    mkdir -p "$FRAMEWORK_MACABI/Versions/A/Resources"
    cp target/universal/ios-macabi/librn_http_server.dylib "$FRAMEWORK_MACABI/Versions/A/RNHttpServer"
    cp include/*.h "$FRAMEWORK_MACABI/Versions/A/Headers/" 2>/dev/null || echo "Info: No headers found in include/"
    
    # 创建符号链接 (必须使用相对路径并在目标目录下执行,否则 xcodebuild 会忽略损坏的链接)
    pushd "$FRAMEWORK_MACABI/Versions" > /dev/null
    ln -s "A" "Current"
    popd > /dev/null
    
    pushd "$FRAMEWORK_MACABI" > /dev/null
    ln -s "Versions/Current/Headers" "Headers"
    ln -s "Versions/Current/Resources" "Resources"
    ln -s "Versions/Current/RNHttpServer" "RNHttpServer"
    popd > /dev/null
    
    # 创建 Info.plist for Frameworks
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
    
    # 生成 XCFramework
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


# 4. 构建 Android 库（可选）
if [ "$AUTO_CONFIRM" = true ]; then
    REPLY="y"
else
    read -p "Do you want to build Android libraries? (y/n): " -n 1 -r
    echo
fi
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo "🤖 Building Android libraries..."
    
    # 检查并设置 NDK
    if [ -z "$ANDROID_NDK_HOME" ]; then
        ANDROID_NDK_HOME="$HOME/Library/Android/sdk/ndk/27.1.12297006"
        if [ ! -d "$ANDROID_NDK_HOME" ]; then
            echo "❌ Android NDK not found at $ANDROID_NDK_HOME"
            echo "Please set ANDROID_NDK_HOME environment variable"
            exit 1
        fi
    fi
    
    echo "  Using NDK at: $ANDROID_NDK_HOME"
    
    # Android 目标架构
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
    
    # 整理输出
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

# 5. 运行 Rust 测试
# ⚠️ 必须 --test-threads=1：所有 server / 事件回调 / CORS 配置都是**进程级全局单例**
#    （CORS_CONFIG、SERVER_HANDLE、RESPONSE_CHANNELS、REQUEST_BODIES...），
#    并行跑时多个测试会同时启停同一批全局状态，互相踩。
#    症状：失败集合每次不同（502 / PoisonError / 状态码断言错乱），
#    且某个测试 panic 时持有全局 Mutex 会毒化后续测试，导致级联失败。
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