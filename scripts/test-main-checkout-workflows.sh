#!/usr/bin/env bash
# Offline behavior fixtures for the actual checkout blocks in the three
# publication workflows. Workflow text is parsed and its run scripts executed.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
python3 - "$ROOT" "$WORK" <<'PY'
import pathlib, sys
try:
    import yaml
except ImportError as exc:
    raise SystemExit(f"PyYAML required: {exc}")
root, work = map(pathlib.Path, sys.argv[1:])
for name in ("dev-bakery.yml", "dev-bread-theme.yml", "package.yml"):
    data = yaml.safe_load((root / ".forgejo/workflows" / name).read_text())
    steps = data["jobs"]["package" if name == "package.yml" else "build"]["steps"]
    step = steps[0]
    (work / (name + ".sh")).write_text(step["run"])
    cleanup = next((s["run"] for s in steps if s.get("name") == "clean owned checkout"), "")
    if cleanup:
        (work / (name + ".cleanup.sh")).write_text(cleanup)
PY
chmod +x "$WORK"/*.sh
ORIGIN="$WORK/origin"
git init -q -b main "$ORIGIN"
git -C "$ORIGIN" config user.name fixture
git -C "$ORIGIN" config user.email fixture@example.invalid
mkdir -p "$ORIGIN/fixture"
echo old > "$ORIGIN/fixture/value"
git -C "$ORIGIN" add . && git -C "$ORIGIN" commit -qm old
OLD=$(git -C "$ORIGIN" rev-parse HEAD)
echo advanced > "$ORIGIN/fixture/new"
git -C "$ORIGIN" add . && git -C "$ORIGIN" commit -qm advanced
git -C "$ORIGIN" update-ref refs/heads/main "$OLD"
git -C "$ORIGIN" update-ref refs/heads/advanced "$(git -C "$ORIGIN" rev-parse HEAD)"
SHA="$OLD"
export GITHUB_REPOSITORY=fixture/repo GITHUB_SHA="$SHA" RUNNER_TEMP="$WORK/tmp"
export GITHUB_REF_NAME=v0.1.0
mkdir -p "$RUNNER_TEMP"
export GITHUB_ENV="$WORK/github-env"
URL="file://$ORIGIN"
# Replace the workflows' URL with a local fixture, without changing the YAML.
for file in "$WORK"/dev-*.yml.sh; do sed -i "s#https://git.breadway.dev/\${GITHUB_REPOSITORY}.git#$URL#" "$file"; done
sed -i "s#https://git.breadway.dev/\${GITHUB_REPOSITORY}.git#$URL#" "$WORK/package.yml.sh"
# Replace pinned helper network fetch with the reviewed local helper copy, but
# retain and exercise the real SHA-256 check in each actual workflow block.
HELPER="$WORK/helper"
git -C "$ROOT" show c6442cabb6c02acd1380dcd0d17189c4e22def03:scripts/ci-checkout.sh > "$HELPER"
for file in "$WORK"/*.sh; do
  sed -i "s#curl -fsSL .* -o \"\$helper\"#cp '$HELPER' \"\$helper\"#" "$file"
done
# Explicit fallback temp root behavior, by executing the extracted dev block
# without RUNNER_TEMP and verifying it selects /tmp.
mkdir -p /tmp/bread-ci-fixture
export RUNNER_TEMP=/tmp/bread-ci-fixture
: > "$GITHUB_ENV"
bash "$WORK/dev-bakery.yml.sh"
fallback=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$GITHUB_ENV")
[[ "$fallback" == /tmp/bread-ci-fixture/bread-ci.* ]]
rm -rf -- "$fallback"
rm -rf -- /tmp/bread-ci-fixture
export RUNNER_TEMP="$WORK/tmp"
for file in "$WORK"/dev-*.yml.sh; do
  : > "$GITHUB_ENV"
  bash "$file"
  dir=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$GITHUB_ENV")
  [ -n "$dir" ] && [ "$(git -C "$dir/src" rev-parse HEAD)" = "$SHA" ]
  [ -f "$dir/src/fixture/value" ] && [ ! -e "$dir/src/fixture/new" ]
  # Environment file is consumed by a separate process, as Forgejo does.
  env "$(cat "$GITHUB_ENV")" bash -c '[ -f "$BREAD_CI_JOB_DIR/src/fixture/value" ]'
  other=$(mktemp -d "$RUNNER_TEMP/bread-ci.XXXXXX")
  [ "$dir" != "$other" ]
  cleanup_file="$WORK/$(basename "$file" .sh).cleanup.sh"
  BREAD_CI_JOB_DIR="$dir" bash "$cleanup_file"
  [ ! -e "$dir" ] && [ -d "$other" ]
  rm -rf "$other"
done
# Invalid event SHA must fail before checkout. Hash corruption and acquisition
# failure must fail closed before the helper can run.
cp "$WORK/dev-bakery.yml.sh" "$WORK/invalid.sh"
GITHUB_SHA=invalid bash "$WORK/invalid.sh" >/dev/null 2>&1 && exit 1 || :
cp "$WORK/helper" "$WORK/corrupt"
printf x >> "$WORK/corrupt"
printf '%s  %s\n' 39d07ff4efe2336fe5eac1f1099590d5427e04111e0f51fc3f905df2cfbd205f "$WORK/corrupt" | sha256sum --check --status && exit 1 || :
# Acquisition failure must stop the real checkout run block before creating a
# checkout directory. Simulate curl failing at the acquisition line.
sed 's#curl -fsSL .* -o "\$helper"#false#' "$WORK/dev-bakery.yml.sh" > "$WORK/acquire-fails.sh"
if bash "$WORK/acquire-fails.sh" >/dev/null 2>&1; then exit 1; fi
# The helper rejects an unknown commit and removes its partially-created tree.
MISSING=$(printf '0%.0s' {1..40})
if "$WORK/helper" "file://$ORIGIN" "$MISSING" "$WORK/missing" >/dev/null 2>&1; then exit 1; fi
[ ! -e "$WORK/missing" ]
# Package workflow has container-private fixed path; use a temp path fixture.
sed -i "s#/home/builder/src#$WORK/package-src#g" "$WORK/package.yml.sh"
# Keep only the checkout section, omitting setup and packaging commands.
python3 - "$WORK/package.yml.sh" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
lines = p.read_text().splitlines()
start = next(i for i, line in enumerate(lines) if line.startswith("helper_commit="))
end = next(i for i, line in enumerate(lines) if line.startswith("cd "))
p.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + "\n".join(lines[start:end]) + "\n")
PY
: > "$GITHUB_ENV"
bash "$WORK/package.yml.sh"
[ "$(git -C "$WORK/package-src" rev-parse HEAD)" = "$SHA" ]
echo "main workflow checkout fixtures passed"
