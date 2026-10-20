#!/usr/bin/env bash
# Offline integration fixtures execute checkout and cleanup run blocks extracted
# from the production YAML. No setup/build/publish suffix is evaluated.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/eco82-fixture.XXXXXX")"
trap 'rm -rf -- "$WORK"' EXIT
python3 - "$ROOT" "$WORK" <<'PY'
import pathlib, sys, yaml
root, work = map(pathlib.Path, sys.argv[1:])
for name in ("dev-bakery.yml", "dev-bread-theme.yml", "package.yml"):
    data = yaml.safe_load((root / ".forgejo/workflows" / name).read_text())
    job = data["jobs"]["package" if name == "package.yml" else "build"]
    steps = job["steps"]
    run = steps[0]["run"]
    if name == "package.yml":
        lines = run.splitlines()
        start = next(i for i, line in enumerate(lines) if line.startswith("helper_commit="))
        end = next(i for i, line in enumerate(lines) if line.startswith("cd "))
        run = "set -euo pipefail\n" + "\n".join(lines[start:end]) + "\n"
    (work / (name + ".sh")).write_text(run)
    cleanup = next((s["run"] for s in steps if s.get("name") == "clean owned checkout"), None)
    if cleanup:
        (work / (name + ".cleanup.sh")).write_text(cleanup)
PY
chmod +x "$WORK"/*.sh
ORIGIN="$WORK/origin"
git init -q -b main "$ORIGIN"
git -C "$ORIGIN" config user.name fixture
git -C "$ORIGIN" config user.email fixture@example.invalid
mkdir -p "$ORIGIN/fixture"
printf 'old\n' > "$ORIGIN/fixture/value"
git -C "$ORIGIN" add . && git -C "$ORIGIN" commit -qm old
OLD=$(git -C "$ORIGIN" rev-parse HEAD)
printf 'new\n' > "$ORIGIN/fixture/value"
printf 'new-only\n' > "$ORIGIN/fixture/new"
git -C "$ORIGIN" add . && git -C "$ORIGIN" commit -qm new
NEW=$(git -C "$ORIGIN" rev-parse HEAD)
[ "$OLD" != "$NEW" ]
URL="file://$ORIGIN"
HELPER="$WORK/reviewed-helper"
HELPER_BLOB=$(git -C "$ROOT" rev-parse c6442cabb6c02acd1380dcd0d17189c4e22def03:scripts/ci-checkout.sh)
git -C "$ROOT" cat-file blob "$HELPER_BLOB" > "$HELPER"
EXPECTED=39d07ff4efe2336fe5eac1f1099590d5427e04111e0f51fc3f905df2cfbd205f
[ "$(sha256sum "$HELPER" | awk '{print $1}')" = "$EXPECTED" ]

prepare_script() {
    local src=$1 dst=$2 helper=${3:-$HELPER}
    local payload="$helper" download="$WORK/$(basename "$dst").download"
    cp "$src" "$dst"
    # Transform only fixture inputs: real workflow block otherwise unchanged.
    sed -i "s#https://git.breadway.dev/\${GITHUB_REPOSITORY}.git#$URL#g" "$dst"
    if [ "$helper" = "$WORK/marker-helper" ]; then
      payload="$WORK/marker-payload"
      cat > "$payload" <<EOF
#!/bin/sh
touch '$WORK/helper-executed'
exit 99
EOF
      chmod +x "$payload"
    fi
    # Corrupt the actual downloaded fixture payload after calculating neither
    # nor adapting the committed digest in the production block.
    cp "$payload" "$download"
    if [ "$helper" = "$WORK/marker-helper" ]; then printf x >> "$download"; fi
    python3 - "$dst" "$download" <<'PY'
import pathlib, sys
p, download = map(pathlib.Path, sys.argv[1:])
text = p.read_text()
lines = text.splitlines()
for i, line in enumerate(lines):
    if line.strip().startswith("curl -fsSL") and "ci-checkout.sh" in line:
        indent = line[:len(line) - len(line.lstrip())]
        lines[i] = f'{indent}cp "{download}" "$helper"'
p.write_text("\n".join(lines) + "\n")
PY
}

run_dev_case() {
    local workflow=$1 sha=$2 label=$3 temp=$4
    local base="$WORK/$label"
    mkdir -p "$temp"
    export GITHUB_REPOSITORY=fixture/repo GITHUB_SHA="$sha" GITHUB_ENV="$base.env"
    export RUNNER_TEMP="$temp"
    : > "$GITHUB_ENV"
    prepare_script "$WORK/$workflow.sh" "$base.run"
    bash "$base.run"
    local dir
    dir=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$GITHUB_ENV")
    [[ "$dir" == "$temp"/bread-ci.* ]] || return 1
    [ "$(git -C "$dir/src" rev-parse HEAD)" = "$sha" ] || return 1
    printf '%s\n' "$dir" > "$base.dir"
}

# Real branch advance: the branch remains at NEW while the event is OLD.
export GITHUB_REPOSITORY=fixture/repo GITHUB_SHA="$OLD"
export RUNNER_TEMP="$WORK/advance-temp" GITHUB_ENV="$WORK/advance.env"
mkdir -p "$RUNNER_TEMP"; : > "$GITHUB_ENV"
prepare_script "$WORK/dev-bakery.yml.sh" "$WORK/advance.run"
bash "$WORK/advance.run"
advance_dir=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$GITHUB_ENV")
[ "$(git -C "$ORIGIN" rev-parse refs/heads/main)" = "$NEW" ]
[ "$(git -C "$advance_dir/src" rev-parse HEAD)" = "$OLD" ]
[ "$(cat "$advance_dir/src/fixture/value")" = old ]
env "$(cat "$GITHUB_ENV")" bash -c '[ -f "$BREAD_CI_JOB_DIR/src/fixture/value" ]'
advance_sibling=$(mktemp -d "$RUNNER_TEMP/bread-ci.XXXXXX")
BREAD_CI_JOB_DIR="$advance_dir" bash "$WORK/dev-bakery.yml.cleanup.sh"
[ ! -e "$advance_dir" ] && [ -d "$advance_sibling" ]
rm -rf -- "$advance_sibling"

# Concurrent extracted workflows use distinct event commits and isolated env
# files/directories. Background statuses are both collected before assertions.
mkdir -p "$WORK/parallel-temp"
(
  export GITHUB_REPOSITORY=fixture/repo GITHUB_SHA="$OLD"
  export RUNNER_TEMP="$WORK/parallel-temp" GITHUB_ENV="$WORK/parallel-old.env"
  : > "$GITHUB_ENV"
  prepare_script "$WORK/dev-bakery.yml.sh" "$WORK/parallel-old.run"
  bash "$WORK/parallel-old.run"
  dir=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$GITHUB_ENV")
  printf '%s\n' "$dir" > "$WORK/parallel-old.dir"
  env "$(cat "$GITHUB_ENV")" bash -c '[ -f "$BREAD_CI_JOB_DIR/src/fixture/value" ]'
  sleep 1
  [ "$(git -C "$dir/src" rev-parse HEAD)" = "$OLD" ] && [ "$(cat "$dir/src/fixture/value")" = old ]
  BREAD_CI_JOB_DIR="$dir" bash "$WORK/dev-bakery.yml.cleanup.sh"
) & p1=$!
(
  export GITHUB_REPOSITORY=fixture/repo GITHUB_SHA="$NEW"
  export RUNNER_TEMP="$WORK/parallel-temp" GITHUB_ENV="$WORK/parallel-new.env"
  : > "$GITHUB_ENV"
  prepare_script "$WORK/dev-bread-theme.yml.sh" "$WORK/parallel-new.run"
  bash "$WORK/parallel-new.run"
  dir=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$GITHUB_ENV")
  printf '%s\n' "$dir" > "$WORK/parallel-new.dir"
  env "$(cat "$GITHUB_ENV")" bash -c '[ -f "$BREAD_CI_JOB_DIR/src/fixture/new" ]'
  sleep 1
  [ "$(git -C "$dir/src" rev-parse HEAD)" = "$NEW" ] && [ "$(cat "$dir/src/fixture/value")" = new ]
  BREAD_CI_JOB_DIR="$dir" bash "$WORK/dev-bread-theme.yml.cleanup.sh"
) & p2=$!
rc1=0; wait "$p1" || rc1=$?
rc2=0; wait "$p2" || rc2=$?
[ "$rc1" -eq 0 ] && [ "$rc2" -eq 0 ]
old_dir=$(cat "$WORK/parallel-old.dir")
new_dir=$(cat "$WORK/parallel-new.dir")
[ "$old_dir" != "$new_dir" ] && [ ! -e "$old_dir" ] && [ ! -e "$new_dir" ]

# Fallback: actually unset RUNNER_TEMP and use a per-invocation temp location.
unset RUNNER_TEMP
export GITHUB_REPOSITORY=fixture/repo GITHUB_SHA="$OLD" GITHUB_ENV="$WORK/fallback.env"
: > "$GITHUB_ENV"
prepare_script "$WORK/dev-bakery.yml.sh" "$WORK/fallback.run"
bash "$WORK/fallback.run"
fallback=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$GITHUB_ENV")
[[ "$fallback" == /tmp/bread-ci.* ]]
BREAD_CI_JOB_DIR="$fallback" bash "$WORK/dev-bakery.yml.cleanup.sh"
[ ! -e "$fallback" ]

# Invalid SHA, missing object, acquisition failure, and corrupt helper all run
# through the actual workflow block and its actual cleanup block.
export RUNNER_TEMP="$WORK/failure-temp" GITHUB_REPOSITORY=fixture/repo
mkdir -p "$RUNNER_TEMP"
failure_case() {
    local name=$1 sha=$2 helper=$3 acquisition=${4:-ok}
    local envfile="$WORK/$name.env" script="$WORK/$name.run"
    export GITHUB_SHA="$sha" GITHUB_ENV="$envfile"
    : > "$envfile"
    prepare_script "$WORK/dev-bakery.yml.sh" "$script" "$helper"
    if [ "$acquisition" = fail ]; then
      python3 - "$script" <<'PY'
import pathlib, sys
p=pathlib.Path(sys.argv[1]); lines=p.read_text().splitlines()
for i,line in enumerate(lines):
    if line.lstrip().startswith('cp "') and ' "$helper"' in line: lines[i]='          false'
p.write_text("\n".join(lines)+"\n")
PY
    fi
    local rc=0
    bash "$script" >"$WORK/$name.log" 2>&1 || rc=$?
    if [ "$rc" -eq 0 ]; then echo "expected $name to fail" >&2; return 1; fi
    local checkout
    checkout=$(sed -n 's/^BREAD_CI_JOB_DIR=//p' "$envfile")
    if [ -n "$checkout" ]; then
      if [ -f "$WORK/dev-bakery.yml.cleanup.sh" ]; then
        BREAD_CI_JOB_DIR="$checkout" bash "$WORK/dev-bakery.yml.cleanup.sh"
      fi
      [ ! -e "$checkout" ] || return 1
    fi
}
failure_case invalid invalid "$HELPER"
MISSING=$(printf '0%.0s' {1..40})
failure_case missing "$MISSING" "$HELPER"
failure_case acquisition "$OLD" "$HELPER" fail
cat > "$WORK/marker-helper" <<EOF
#!/bin/sh
touch '$WORK/helper-executed'
exit 99
EOF
chmod +x "$WORK/marker-helper"
# Hash mismatch must stop before helper execution; invoke actual block with a
# corrupt payload placed at its fetched-helper path and preserve actual digest.
failure_case corrupt "$OLD" "$WORK/marker-helper"
[ ! -e "$WORK/helper-executed" ]

# Package checkout uses fixed container-private path. For fixture safety, map
# only that path into WORK; run no pacman/setup or packaging suffix.
package_case() {
  local name=$1 sha=$2 helper=$3 acquire=${4:-ok}
  local src="$WORK/$name/package.yml.sh" envfile="$WORK/$name.env"
  mkdir -p "$WORK/$name"
  cp "$WORK/package.yml.sh" "$src"
  sed -i "s#/home/builder/src#$WORK/$name/src#g; s#https://git.breadway.dev/\${GITHUB_REPOSITORY}.git#$URL#g" "$src"
  local payload="$helper" download="$WORK/$name/downloaded-helper"
  if [ "$helper" = "$WORK/marker-helper" ]; then
    payload="$WORK/package-marker"
    printf '#!/bin/sh\ntouch %q\nexit 99\n' "$WORK/helper-executed" > "$payload"
  fi
  cp "$payload" "$download"
  if [ "$helper" = "$WORK/marker-helper" ]; then printf x >> "$download"; fi
  python3 - "$src" "$download" <<'PY'
import pathlib, sys
p, download = map(pathlib.Path, sys.argv[1:])
lines = p.read_text().splitlines()
for i, line in enumerate(lines):
    if line.strip().startswith("curl -fsSL") and "ci-checkout.sh" in line:
        indent = line[:len(line) - len(line.lstrip())]
        lines[i] = f'{indent}cp "{download}" "$helper"'
p.write_text("\n".join(lines) + "\n")
PY
  if [ "$acquire" = fail ]; then
    python3 - "$src" <<'PY'
import pathlib, sys
p=pathlib.Path(sys.argv[1]); lines=p.read_text().splitlines()
for i,line in enumerate(lines):
    if line.lstrip().startswith('cp "') and ' "$helper"' in line: lines[i]='          false'
p.write_text("\n".join(lines)+"\n")
PY
  fi
  export GITHUB_SHA="$sha" GITHUB_REPOSITORY=fixture/repo
  if bash "$src" >"$WORK/$name.log" 2>&1; then
    [ "$(git -C "$WORK/$name/src" rev-parse HEAD)" = "$sha" ] || return 1
  else
    [ ! -e "$WORK/$name/src" ] || return 1
  fi
  # The package job uses its private container source path and has no cleanup
  # step. Remove only the fixture-owned container directory after assertions.
  rm -rf -- "$WORK/$name/src"
}
package_case package-good "$OLD" "$HELPER"
package_case package-missing "$MISSING" "$HELPER"
package_case package-corrupt "$OLD" "$WORK/marker-helper"
[ ! -e "$WORK/helper-executed" ]
package_case package-acquisition "$OLD" "$HELPER" fail

# Mutation check: removing the digest verification from any workflow fixture
# must allow the marker payload to run, which this test must reject.
for workflow in dev-bakery.yml dev-bread-theme.yml package.yml; do
  mut="$WORK/mut-$workflow.sh"
  prepare_script "$WORK/$workflow.sh" "$mut" "$WORK/marker-helper"
  sed -i '/sha256sum --check/d' "$mut"
  export GITHUB_SHA="$OLD" GITHUB_REPOSITORY=fixture/repo
  if [ "$workflow" = package.yml ]; then
    sed -i "s#/home/builder/src#$WORK/mut-package-src#g" "$mut"
    mkdir -p "$WORK/package-mut"
    cat > "$WORK/package-mut/marker-helper" <<EOF
#!/bin/sh
touch '$WORK/helper-executed'
exit 99
EOF
    chmod +x "$WORK/package-mut/marker-helper"
    cp "$WORK/package-mut/marker-helper" "$WORK/package-mut/downloaded-helper"
    printf x >> "$WORK/package-mut/downloaded-helper"
    python3 - "$mut" "$WORK/package-mut/downloaded-helper" <<'PY'
import pathlib, sys
p, download = map(pathlib.Path, sys.argv[1:]); lines=p.read_text().splitlines()
for i,line in enumerate(lines):
    if line.strip().startswith("curl -fsSL"): lines[i]=line[:len(line)-len(line.lstrip())]+f'cp "{download}" "$helper"'
p.write_text("\n".join(lines)+"\n")
PY
  else
    export RUNNER_TEMP="$WORK/mutation-temp" GITHUB_ENV="$WORK/mut.env"
    mkdir -p "$RUNNER_TEMP"; : > "$GITHUB_ENV"
    cat > "$WORK/mutation-temp/marker-helper" <<EOF
#!/bin/sh
touch '$WORK/helper-executed'
exit 99
EOF
    chmod +x "$WORK/mutation-temp/marker-helper"
    cp "$WORK/mutation-temp/marker-helper" "$WORK/mutation-temp/downloaded-helper"
    printf x >> "$WORK/mutation-temp/downloaded-helper"
    python3 - "$mut" "$WORK/mutation-temp/downloaded-helper" <<'PY'
import pathlib, sys
p, download = map(pathlib.Path, sys.argv[1:]); lines=p.read_text().splitlines()
for i,line in enumerate(lines):
    if line.strip().startswith("curl -fsSL"): lines[i]=line[:len(line)-len(line.lstrip())]+f'cp "{download}" "$helper"'
p.write_text("\n".join(lines)+"\n")
PY
  fi
  rm -f "$WORK/helper-executed"
  if bash "$mut" >/dev/null 2>&1 && [ -e "$WORK/helper-executed" ]; then
    echo "digest-removal mutation was not caught for $workflow" >&2
    exit 1
  fi
  [ -e "$WORK/helper-executed" ] || { echo "mutation fixture did not reach marker for $workflow" >&2; exit 1; }
done
echo "main workflow checkout fixtures passed"
