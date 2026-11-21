#!/bin/sh
# Run in a privileged pod that sees the machine's processes. Sets, in each
# timada pod's own network, how its packets to the pods of other regions
# leave: after a delay, or not at all.
#   delay.sh off | on EU_MS US_MS [LOSS_PERCENT] | cut REGION
# /s/pods.txt: name ip region, one pod a line. Regions r0 and r1 are close
# (EU_MS one way), r2 is far from both (US_MS one way).
mode=$1
pid_of() { for p in $(pgrep -x timada-dashboard); do tr '\0' '\n' < /proc/$p/environ | grep -qx "HOSTNAME=$1" && echo $p && return; done; }
while read name ip region; do
  pid=$(pid_of $name); [ -z "$pid" ] && echo "no process for $name" && continue
  in_pod="nsenter -t $pid -n"
  $in_pod tc qdisc del dev eth0 root 2>/dev/null
  [ "$mode" = off ] && continue
  $in_pod tc qdisc add dev eth0 root handle 1: prio bands 4 priomap 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  if [ "$mode" = on ]; then
    loss=${4:+loss $4%}
    $in_pod tc qdisc add dev eth0 parent 1:2 handle 20: netem delay ${2}ms $loss
    $in_pod tc qdisc add dev eth0 parent 1:3 handle 30: netem delay ${3}ms $loss
  else
    $in_pod tc qdisc add dev eth0 parent 1:4 handle 40: netem loss 100%
  fi
  while read other oip oregion; do
    [ "$oregion" = "$region" ] && continue
    if [ "$mode" = on ]; then
      case "$region$oregion" in r0r1|r1r0) band=2 ;; *) band=3 ;; esac
    else
      [ "$region" = "$2" ] || [ "$oregion" = "$2" ] || continue
      band=4
    fi
    $in_pod tc filter add dev eth0 parent 1: protocol ip u32 match ip dst $oip/32 flowid 1:$band
  done < /s/pods.txt
done < /s/pods.txt
echo "delay: $*"
