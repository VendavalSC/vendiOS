#!/usr/bin/env bash

# vendiOS live environment shell config

# colors
PURPLE='\[\e[38;2;203;166;247m\]'
BOLD='\[\e[1m\]'
RESET='\[\e[0m\]'

PS1="${BOLD}${PURPLE}vendios${RESET} \w \$ "

alias ls='ls --color=auto'
alias ll='ls -la --color=auto'
alias grep='grep --color=auto'
alias install='vendi-install'
alias fetch='fastfetch'

export EDITOR=vim
export TERM=xterm-256color
