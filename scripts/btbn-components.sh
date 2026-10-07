#!/bin/bash
# Lists the components BtbN's FFmpeg-Builds scripts build for one target, variant and addins,
# using the scripts' own logic (generate.sh's dependency walk and each script's
# ffbuild_enabled): one tab-separated line a script, with where its source comes from.
#   btbn-components.sh <FFmpeg-Builds checkout> win64 lgpl-shared 8.1
set -e
shopt -s globstar
cd "$1"
shift
source util/vars.sh "$@"

resolvestage() {
    [[ -d "$1" ]] && local SCRIPTDIR=("$1") || local SCRIPTDIR=(scripts.d/??-"$1")
    if [[ -d "${SCRIPTDIR[0]}" ]]; then
        echo scripts.d/??-"${1}"
    else
        echo scripts.d/??-"${1}.sh"
    fi
}

get_stagedeps() {
    [[ -d "$1" ]] && local SCRIPTDIR=("$1") || local SCRIPTDIR=(scripts.d/??-"$1")
    if [[ -d "${SCRIPTDIR[0]}" ]]; then
        RESDEPS=()
        for SUBSCRIPT in "${SCRIPTDIR[0]}"/*.sh; do
            RESDEPS+=( $(get_stagedeps "${SUBSCRIPT}") )
        done
        tr ' ' '\n' <<< "${RESDEPS[@]}" | sort -u
    else
        [[ -f "$1" ]] && SCRIPT=("$1") || SCRIPT=(scripts.d/??-"${1}.sh")
        SCRIPT="${SCRIPT[0]}"
        (
            SELF="$SCRIPT"
            STAGENAME="$(basename "$SCRIPT" | sed 's/.sh$//')"
            source util/dl_functions.sh
            source "$SCRIPT"
            ffbuild_enabled || exit 0
            ffbuild_depends
        )
    fi
}

get_stagedeps_recursive_internal() {
    local CDEPS=($(get_stagedeps "$1"))
    for CDEP in "${CDEPS[@]}"; do
        get_stagedeps_recursive_internal "$CDEP"
    done
    printf '%s\n' "${CDEPS[@]}"
}

get_stagedeps_recursive() {
    declare -A ALREADY_PRINTED
    for CDEP in $(get_stagedeps_recursive_internal "$1"); do
        if ! [[ -v ALREADY_PRINTED["$CDEP"] ]]; then
            echo "$CDEP"
            ALREADY_PRINTED["$CDEP"]="1"
        fi
    done
}

ENTRYSCRIPT="$(ls -1d scripts.d/* | tail -n 1)"
for DEP in $(get_stagedeps_recursive "$ENTRYSCRIPT"); do
    STAGE="$(resolvestage "$DEP")"
    if [[ -d "$STAGE" ]]; then SCRIPTS=("$STAGE"/??-*.sh); else SCRIPTS=("$STAGE"); fi
    for SCRIPT in "${SCRIPTS[@]}"; do
        (
            SELF="$SCRIPT"
            STAGENAME="$(basename "$SCRIPT" | sed 's/.sh$//')"
            source util/dl_functions.sh
            source "$SCRIPT"
            ffbuild_enabled || exit 0
            printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$SCRIPT" "${SCRIPT_SKIP:-}" \
                "${SCRIPT_REPO:-}" "${SCRIPT_COMMIT:-}" "${SCRIPT_REV:-}" \
                "${SCRIPT_REPO2:-}" "${SCRIPT_COMMIT2:-}" "${SCRIPT_REPO3:-}" "${SCRIPT_COMMIT3:-}"
        )
    done
done
