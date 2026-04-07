const fs = require('fs');
const path = require('path');

/**
 * This script fixes symlinks in the RNHttpServer.xcframework for Mac Catalyst.
 * Yarn v1 often strips or corrupts symlinks in node_modules, which breaks CocoaPods.
 */

function fixSymlinks() {
  const userAgent = process.env.npm_config_user_agent || '';
  const isYarnV1 = userAgent.includes('yarn/1.');

  if (!isYarnV1) {
    // We only force-fix for Yarn v1, as other managers handle this better.
    return;
  }

  console.log('[RNHttpServer] Yarn v1 detected. Fixing Mac Catalyst framework symlinks...');

  const frameworkPath = path.resolve(
    __dirname,
    '../ios/Frameworks/RNHttpServer.xcframework/ios-arm64_x86_64-maccatalyst/RNHttpServer.framework'
  );

  if (!fs.existsSync(frameworkPath)) {
    console.log('[RNHttpServer] Mac Catalyst framework not found, skipping.');
    return;
  }

  const versionsAPath = path.join(frameworkPath, 'Versions/A');
  if (!fs.existsSync(versionsAPath)) {
    console.log('[RNHttpServer] Versions/A not found, xcframework might be incomplete.');
    return;
  }

  try {
    // Change directory to the framework root
    process.chdir(frameworkPath);

    // Remove existing files/links that should be symlinks
    const toRemove = ['Headers', 'Resources', 'RNHttpServer', 'Info.plist'];
    toRemove.forEach((file) => {
      if (fs.existsSync(file)) {
        fs.rmSync(file, { recursive: true, force: true });
      }
    });

    const toLink = ['Headers', 'Resources', 'RNHttpServer'];
    toLink.forEach((file) => {
      // Create symlink: link -> Versions/A/link
      try {
        fs.symlinkSync(`Versions/A/${file}`, file);
        console.log(`[RNHttpServer] Created symlink: ${file} -> Versions/A/${file}`);
      } catch (e) {
        console.error(`[RNHttpServer] Failed to create symlink for ${file}:`, e.message);
      }
    });

    console.log('[RNHttpServer] Symlinks fixed successfully.');
  } catch (err) {
    console.error('[RNHttpServer] Error while fixing symlinks:', err.message);
  }
}

fixSymlinks();
