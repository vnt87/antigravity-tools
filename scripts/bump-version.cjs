const fs = require('fs');
const path = require('path');

const versionType = process.argv[2] || 'patch'; // 'patch', 'minor', 'major'

const rootDir = path.resolve(__dirname, '..');
const packageJsonPath = path.join(rootDir, 'package.json');
const tauriConfPath = path.join(rootDir, 'src-tauri', 'tauri.conf.json');
const cargoTomlPath = path.join(rootDir, 'src-tauri', 'Cargo.toml');

function bumpVersion(version, type) {
    const parts = version.split('.').map(Number);
    if (type === 'major') {
        parts[0]++;
        parts[1] = 0;
        parts[2] = 0;
    } else if (type === 'minor') {
        parts[1]++;
        parts[2] = 0;
    } else {
        parts[2]++;
    }
    return parts.join('.');
}

// 1. Update package.json
console.log(`Reading ${packageJsonPath}...`);
const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));
const oldVersion = packageJson.version;
const newVersion = bumpVersion(oldVersion, versionType);

console.log(`Bumping version from ${oldVersion} to ${newVersion} (${versionType})`);

packageJson.version = newVersion;
fs.writeFileSync(packageJsonPath, JSON.stringify(packageJson, null, 2) + '\n');
console.log(`Updated package.json`);

// 2. Update tauri.conf.json
console.log(`Reading ${tauriConfPath}...`);
const tauriConf = JSON.parse(fs.readFileSync(tauriConfPath, 'utf8'));
tauriConf.version = newVersion;
fs.writeFileSync(tauriConfPath, JSON.stringify(tauriConf, null, 2) + '\n');
console.log(`Updated tauri.conf.json`);

// 3. Update Cargo.toml
console.log(`Reading ${cargoTomlPath}...`);
let cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
// Regex to match version = "x.y.z" inside [package] section
// This is a simple regex and assumes standard formatting.
// It looks for the first occurrence of version = "..." which is usually the package version.
const versionRegex = /^version\s*=\s*"[^"]+"/m;
if (versionRegex.test(cargoToml)) {
    cargoToml = cargoToml.replace(versionRegex, `version = "${newVersion}"`);
    fs.writeFileSync(cargoTomlPath, cargoToml);
    console.log(`Updated Cargo.toml`);
} else {
    console.error(`Could not find version string in Cargo.toml`);
    process.exit(1);
}

console.log(`::set-output name=new_version::${newVersion}`);
console.log(newVersion);
