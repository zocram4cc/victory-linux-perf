# Shared by the hooks in this folder: every commit of this repository is
# made as `developer <developer@localhost>`, unsigned, and nothing that names a
# contributor goes in. Sourced, not run.
#
# The terms that must not appear (names, addresses, hosts) are kept outside
# the repository, one per line, in .git/info/identity-denylist; lines starting
# with # are ignored. Without that file only the identity itself is checked.

EXPECTED_IDENT="developer <developer@localhost>"

identity_denylist() {
    f="$(git rev-parse --git-path info/identity-denylist)"
    [ -f "$f" ] || return 1
    grep -vE '^[[:space:]]*(#|$)' "$f"
}

# Text on stdin containing a denied term: prints the matching lines, fails.
identity_text_ok() {
    terms="$(identity_denylist)" || return 0
    [ -n "$terms" ] || return 0
    hits="$(grep -inwF -e "$(printf '%s' "$terms")" 2>/dev/null)" || return 0
    printf '%s\n' "$hits" | head -5 >&2
    return 1
}

identity_strip_date() {
    sed -E 's/ [0-9]+ [+-][0-9]{4}$//'
}

# The author and committer git would use now, and signing off.
identity_config_ok() {
    ok=0
    author="$(git var GIT_AUTHOR_IDENT | identity_strip_date)"
    committer="$(git var GIT_COMMITTER_IDENT | identity_strip_date)"
    if [ "$author" != "$EXPECTED_IDENT" ] || [ "$committer" != "$EXPECTED_IDENT" ]; then
        echo "identity: commits here are made as '$EXPECTED_IDENT' (author is '$author', committer '$committer')" >&2
        echo "identity: git config --local user.name developer && git config --local user.email developer@localhost" >&2
        ok=1
    fi
    if [ "$(git config --bool commit.gpgsign)" = "true" ]; then
        echo "identity: commits here are unsigned: git config --local commit.gpgsign false" >&2
        ok=1
    fi
    return $ok
}

# One commit: its author and committer, no signature, and its message.
identity_commit_ok() {
    c="$1"
    ok=0
    who="$(git log -1 --format='%an <%ae>|%cn <%ce>' "$c")"
    if [ "$who" != "$EXPECTED_IDENT|$EXPECTED_IDENT" ]; then
        echo "identity: $(git log -1 --format='%h %s' "$c"): made by '$who'" >&2
        ok=1
    fi
    if git cat-file commit "$c" | grep -q '^gpgsig'; then
        echo "identity: $(git log -1 --format='%h %s' "$c"): signed" >&2
        ok=1
    fi
    return $ok
}

# One commit's author, committer and message hold no denied term.
identity_commit_clean() {
    c="$1"
    if ! git log -1 --format='%an <%ae>%n%cn <%ce>%n%B' "$c" | identity_text_ok; then
        echo "identity: $(git log -1 --format='%h %s' "$c"): names a contributor (see above)" >&2
        return 1
    fi
    return 0
}
