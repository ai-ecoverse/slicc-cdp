set -euo pipefail
if command -v gh >/dev/null; then echo "gh: PASS $(gh --version | head -n 1)"; exit 0; fi
case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) exit 1 ;;
esac
version=2.100.0
file="gh_${version}_linux_${arch}.tar.gz"
dir="$RUNNER_TEMP/cert-gh"
mkdir -p "$dir"
cd "$dir"
curl -fsSL -O "https://github.com/cli/cli/releases/download/v${version}/${file}" -O "https://github.com/cli/cli/releases/download/v${version}/gh_${version}_checksums.txt"
grep "  ${file}$" "gh_${version}_checksums.txt" | sha256sum -c -
tar -xzf "$file" --strip-components=2 "gh_${version}_linux_${arch}/bin/gh"
printf '%s\n' "$dir" >> "$GITHUB_PATH"
echo "gh: PASS $version"
