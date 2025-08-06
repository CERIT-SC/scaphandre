#!/bin/bash

scaphandre "$@" &
pid=$!

trap 'echo "SIGTERM received. Shutting down Scaphandre..."; kill -TERM $pid' TERM
trap 'echo "SIGINT received. Shutting down Scaphandre..."; kill -INT $pid' INT

wait $pid