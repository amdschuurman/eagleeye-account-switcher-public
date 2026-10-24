#!/usr/bin/env bash
# Blocks personal data and secrets from entering the repository.
#   tools/check-private.sh                  check staged files (the pre-commit hook runs this)
#   tools/check-private.sh --all            check every tracked file
#   tools/check-private.sh --secrets-only   staged files, secrets only (for a private repo that may hold names)
# Terms come from $EAAS_DENYLIST, else private/denylist.txt (one term per line, # comments), matched
# case-insensitively as whole words. Real-looking email addresses and user folder paths are always blocked; a line
# that carries "check-private: ok" is exempt from the path rule (for invented test paths). Private keys and tokens
# are blocked in every mode.
set -uo pipefail
REPO="$(git rev-parse --show-toplevel)"
cd "$REPO" || exit 1
MODE="${1:-}"
if [ "$MODE" = "--all" ]; then
  mapfile -t FILES < <(git ls-files)
else
  mapfile -t FILES < <(git diff --cached --name-only --diff-filter=ACMR)
fi
FILES=($(printf '%s\n' "${FILES[@]}" | grep -vE '\.(png|ico|webp|jpg|lock)$|package-lock\.json$' || true))
[ ${#FILES[@]} -eq 0 ] && exit 0

read_file() { if [ "$1" = "--all" ]; then cat -- "$2"; else git show ":$2"; fi; }
FAIL=0
DENY="${EAAS_DENYLIST:-$REPO/private/denylist.txt}"
TERMS=""
if [ -f "$DENY" ] && [ "$MODE" != "--secrets-only" ]; then TERMS="$(grep -vE '^\s*(#|$)' "$DENY")"; fi
EMAIL='[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}'
SAFE_EMAIL='@(b\.com|y\.com|example\.(com|org|invalid)|[a-z0-9.-]+\.invalid|anthropic\.com)'
SECRET='(PRIVATE KEY-----|secret key|TAURI_SIGNING_PRIVATE_KEY=[A-Za-z0-9+/]|gh[pousr]_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9_-]{20,}|xox[bp]-|AKIA[0-9A-Z]{16})'  # check-private: ok
for f in "${FILES[@]}"; do
  body="$(read_file "$MODE" "$f" 2>/dev/null)" || continue
  hits="$(printf '%s\n' "$body" | grep -n -E "$SECRET" | grep -v "check-private: ok" | cut -d: -f1 | tr '\n' ' ')"
  if [ -n "$hits" ]; then echo "secret-shaped text in $f, lines: $hits"; FAIL=1; fi
  [ "$MODE" = "--secrets-only" ] && continue
  if [ -n "$TERMS" ]; then
    hits="$(printf '%s\n' "$body" | grep -n -i -w -F -f <(printf '%s\n' "$TERMS") | cut -d: -f1 | tr '\n' ' ')"
    if [ -n "$hits" ]; then echo "personal term in $f, lines: $hits"; FAIL=1; fi
  fi
  hits="$(printf '%s\n' "$body" | grep -n -E "$EMAIL" | grep -v -E "$SAFE_EMAIL" | grep -v -E '[*]{3}@' | cut -d: -f1 | tr '\n' ' ')"
  if [ -n "$hits" ]; then echo "email address in $f, lines: $hits"; FAIL=1; fi
  hits="$(printf '%s\n' "$body" | grep -n -i -E '(C:\\\\?Users\\\\?[A-Za-z]|/mnt/c/Users/|/home/[a-z]+/)' | grep -v "check-private: ok" | cut -d: -f1 | tr '\n' ' ')"
  if [ -n "$hits" ]; then echo "user folder path in $f, lines: $hits"; FAIL=1; fi
done
# Term values are never printed, only file and line numbers.
if [ $FAIL -ne 0 ]; then echo "check-private: blocked (edit the lines above, or the denylist if a term is wrong)"; exit 1; fi
exit 0
