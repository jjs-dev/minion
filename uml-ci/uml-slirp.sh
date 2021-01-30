#!/bin/bash

stdbuf -i 0 -o 0 tee slirp_in.log | slirp-fullbolt 'host addr 10.0.2.2' 'redir tcp 2224 10.0.2.15:22' | stdbuf -i 0 -o 0 tee slirp_out.log
