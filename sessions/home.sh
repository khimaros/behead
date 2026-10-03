# sourced by the session scripts: applications get a home of the session's
# own, BEHEAD_HOME or else .behead in the home of whoever runs behead, so
# their settings and data stay apart from that user's. the exported
# BEHEAD_HOME keeps a script run from another one on the same home
export BEHEAD_HOME="${BEHEAD_HOME:-$HOME/.behead}"
export HOME="$BEHEAD_HOME"
mkdir -p "$HOME"
