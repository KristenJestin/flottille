#!/usr/bin/env bash
# Refuses a commit made directly on main, dev, a stable line stable/X.Y or the next version
# next/X.Y (see AGENTS.md, Git and pull requests).
case "$1" in
  main|dev|stable/*|next/*) echo "Never commit on $1 directly: create a branch first." >&2; exit 1 ;;
esac
