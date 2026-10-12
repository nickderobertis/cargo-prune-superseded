#!/usr/bin/env bash
# What crates.io serves right now for the one artifact this repository publishes.
#
#   usage: release-probe.sh crate:<name>
#
# Exactly three answers, and keeping the last two apart is the point:
#
#   * exit 0 with one line on stdout — the version crates.io serves now;
#   * exit 0 with nothing on stdout — crates.io has no release of it yet (a 404, or
#     `"max_stable_version":null`);
#   * a non-zero exit with the reason on stderr — not answered: an unreadable body, a
#     failed request, or an identifier release-targets.toml does not declare.
#
# A consumer holds on "not answered" and never reads it as "not released", so nothing
# here degrades a registry that did not answer into an empty answer.
#
# The identifiers it answers for are the `[[target]]` ids release-targets.toml declares,
# resolved from this script's own location rather than `$PWD`. It reads that file line
# by line (bash has no TOML reader); `onevcs release declaration` holds the document's
# full shape. It is spawned directly, from any directory, with only PATH and HOME, and
# needs no credential: crates.io is read unauthenticated, once, bounded by curl's
# --max-time. crates/repo-checks/tests/release_probe.rs drives every answer against
# a stub `curl`.

set -euo pipefail

# crates.io answers 403 to a request that does not say who is asking.
readonly USER_AGENT="cargo-prune-superseded-release-probe (+https://github.com/nickderobertis/cargo-prune-superseded)"
readonly CONNECT_TIMEOUT=10
readonly MAX_TIME=25

# Not answered: the reason and what to do, on stderr, because stdout is the answer.
refuse() {
    printf 'release-probe.sh: %s\n' "$1" >&2
    printf 'ACTION: %s\n' "$2" >&2
    exit "${3:-1}"
}

probe_dir="${BASH_SOURCE[0]%/*}"
[ "$probe_dir" = "${BASH_SOURCE[0]}" ] && probe_dir="."
# llmlint: ignore[changed_behavior_has_e2e] unreachable from a run: the shell just resolved this script through probe_dir, so probe_dir/.. resolves too; the guard keeps a broken checkout's refusal in the probe's three-answer contract rather than a bare cd error.
cd -- "$probe_dir/.." 2>/dev/null || refuse \
    "cannot reach the repository root from ${BASH_SOURCE[0]}" \
    "run this script from a checkout of the repository, at scripts/release-probe.sh"
readonly DECLARATION="release-targets.toml"

if [ $# -ne 1 ]; then
    refuse "takes exactly one registry-qualified identifier, and was given $#" \
        "run 'release-probe.sh crate:<name>'" 2
fi
identifier="$1"

for tool in curl grep; do
    command -v "$tool" >/dev/null 2>&1 || refuse \
        "$tool is not on PATH, so crates.io cannot be read" \
        "install $tool, or put it on the PATH this probe is spawned with"
done

trim() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    printf '%s' "${value%"${value##*[![:space:]]}"}"
}

[ -r "$DECLARATION" ] || refuse \
    "$PWD/$DECLARATION is missing or unreadable, so nothing is declared" \
    "restore release-targets.toml from git"

targets=""
table=""
matched=1
while IFS= read -r line || [ -n "$line" ]; do
    line="$(trim "$line")"
    case "$line" in
    "" | "#"*) continue ;;
    "[["*) table="${line%%]]*}]]"; continue ;;
    "["*) table="${line%%]*}]"; continue ;;
    esac
    [ "$table" = "[[target]]" ] || continue
    case "$line" in
    id | id[!A-Za-z0-9_-]*) ;;
    *) continue ;;
    esac
    value="$(trim "${line#id}")"
    case "$value" in "="*) ;; *) continue ;; esac
    value="$(trim "${value#=}")"
    # A quoted string, then nothing but an optional comment.
    rest="${value#\"}"
    rest="${rest#*\"}"
    rest="$(trim "$rest")"
    case "$value" in
    '"'*'"'*) ;;
    *) refuse "$DECLARATION gives a [[target]] the id $value, which is not a quoted string" \
        "spell every id as a quoted crate:<name>" ;;
    esac
    case "$rest" in
    "" | "#"*) ;;
    *) refuse "$DECLARATION gives a [[target]] the id $value, with '$rest' after the string" \
        "leave nothing but a comment after a target's quoted id" ;;
    esac
    value="${value#\"}"
    value="${value%%\"*}"
    case "$value" in
    [a-z]*:[A-Za-z0-9]* ) ;;
    *) refuse "$DECLARATION declares the id '$value', which is not <registry>:<name>" \
        "spell every id as <registry>:<name>, e.g. crate:cargo-prune-superseded" ;;
    esac
    case "$value" in
    *[!A-Za-z0-9:._-]*) refuse "$DECLARATION declares the id '$value', which holds a character no registry name has" \
        "spell every id as <registry>:<name> with letters, digits, '.', '_' and '-'" ;;
    esac
    targets="$targets $value"
    [ "$value" = "$identifier" ] && matched=0
done <"$DECLARATION"

if [ "$matched" -ne 0 ]; then
    refuse "'$identifier' is not a release target this repository declares" \
        "ask for one of:${targets:- (none declared)} — or declare it as a [[target]] in $DECLARATION"
fi

# The identifier becomes a URL path segment, so only a name crates.io could serve passes.
case "$identifier" in
crate:*) name="${identifier#crate:}" ;;
*) refuse "'$identifier' names no registry this probe reads" \
    "declare crates.io targets only, as crate:<name>" ;;
esac
case "$name" in
"" | [!A-Za-z0-9]* | *[!A-Za-z0-9_-]*)
    refuse "'$identifier' is not a name crates.io serves" "spell the name exactly as crates.io does" ;;
esac

url="https://crates.io/api/v1/crates/$name"
rc=0
out="$(curl -q -sS --connect-timeout "$CONNECT_TIMEOUT" --max-time "$MAX_TIME" \
    -A "$USER_AGENT" -w '\n%{http_code}' -- "$url")" || rc=$?
if [ "$rc" -ne 0 ]; then
    refuse "curl exited $rc reading $url, so crates.io did not answer" \
        "re-ask once crates.io is reachable; nothing about a release has been established"
fi
status="${out##*$'\n'}"
body="${out%$'\n'*}"

case "$status" in
200) ;;
404) exit 0 ;;
[0-9][0-9][0-9]) refuse "crates.io answered $status for $identifier, so it cannot say what is released" \
    "re-ask once crates.io answers; nothing about a release has been established" ;;
*) refuse "curl reported '$status' rather than an HTTP status for $url" \
    "re-ask once crates.io answers; nothing about a release has been established" ;;
esac

# `max_stable_version` is what `cargo install` resolves: the greatest version crates.io
# serves, yanked versions and prereleases excluded; `null` means it serves none. The body
# must be a crate document naming this crate and carrying exactly one such field, whose
# value is null or a stable semantic version — anything else is not answered.
# llmlint: ignore-block[boundary_inputs_validated, contracts_have_one_source_or_a_drift_gate] the probe contract allows only bash, curl and grep (no JSON parser), and crates.io publishes no schema to derive one from, so the document is held to the live v1 shape instead: the crate object opening the body with this crate's id, exactly one max_stable_version anywhere, and its value null or a stable semantic version; every other body is refused as not answered.
case "$body" in
"{"*"}") ;;
*) refuse "crates.io answered 200 for $identifier with a body that is not a JSON object" \
    "re-ask once crates.io answers a crate document; nothing about a release has been established" ;;
esac
case "$body" in
"{\"crate\":{\"id\":\"$name\","*) ;;
*) refuse "crates.io answered 200 for $identifier with a document that is not this crate's" \
    "re-ask once crates.io answers this crate's document; nothing about a release has been established" ;;
esac
fields="$(printf '%s' "$body" | grep -o '"max_stable_version":\(null\|"[^"]*"\)')" || fields=""
if [ -z "$fields" ] || [ "$(printf '%s\n' "$fields" | grep -c .)" -ne 1 ]; then
    refuse "crates.io answered 200 for $identifier without exactly one readable max_stable_version" \
        "re-ask once crates.io answers a crate document; nothing about a release has been established"
fi
value="${fields#\"max_stable_version\":}"
[ "$value" = null ] && exit 0
version="${value#\"}"
version="${version%\"}"
readonly STABLE_SEMVER='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
if ! printf '%s\n' "$version" | grep -Eq "$STABLE_SEMVER"; then
    refuse "crates.io answered '$version' for $identifier, which is not a stable semantic version" \
        "re-ask once crates.io answers a version; nothing about a release has been established"
fi
# llmlint: ignore-end[boundary_inputs_validated, contracts_have_one_source_or_a_drift_gate]
printf '%s\n' "$version"
