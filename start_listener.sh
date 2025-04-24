#!/bin/sh

while true; do
  echo "Starting puncher listen mode..."
  ./puncher listen --public 0.0.0.0:18605 --control 0.0.0.0:12345
  echo "puncher exited. Restarting in 2 seconds..."
  sleep 2
done
