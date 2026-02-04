# Release Process

## Creating a Release

1. **Update the version** in `Cargo.toml`:
   ```toml
   version = "0.2.0"
   ```

2. **Commit the version bump**:
   ```bash
   git add Cargo.toml Cargo.lock
   git commit -m "Release v0.2.0"
   ```

3. **Create and push a tag**:
   ```bash
   git tag v0.2.0
   git push origin master --tags
   ```

4. **Wait for CI** - GitHub Actions will automatically:
   - Build binaries for Linux (x86_64) and macOS (x86_64, ARM)
   - Create a GitHub release with the tag name
   - Attach the compiled binaries as release assets

5. **Verify the release** at https://github.com/yonaskolb/verify/releases

## Version Bumping

Follow [Semantic Versioning](https://semver.org/):
- **Patch** (0.1.0 → 0.1.1): Bug fixes, no API changes
- **Minor** (0.1.0 → 0.2.0): New features, backwards compatible
- **Major** (0.1.0 → 1.0.0): Breaking changes

## Build Targets

The release workflow builds for:
- `x86_64-unknown-linux-gnu` - Linux (Intel/AMD)
- `x86_64-apple-darwin` - macOS (Intel)
- `aarch64-apple-darwin` - macOS (Apple Silicon)
