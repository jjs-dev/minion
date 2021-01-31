#!/bin/bash

exec slirp-fullbolt 'host addr 10.0.2.2' 'redir tcp 2224 10.0.2.15:22'
