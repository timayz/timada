#!/bin/sh
# Run in a pod no delay applies to: the times are the node's own.
#   measure.sh LABEL COUNT [PODS]
# through the first pod of each region, or the pods named.
label=$1; n=$2
stats() { sort -n | awk '{a[NR]=$1} END {if (NR==0) {print "none"; exit} printf "median %.0f ms, 9 in 10 under %.0f ms, slowest %.0f ms (%d)", a[int((NR+1)/2)]*1000, a[int(NR*0.9+0.5)]*1000, a[NR]*1000, NR}'; }
while read name ip region; do
  case " ${3:-timada-0 timada-3 timada-6} " in *" $name "*) ;; *) continue ;; esac
  B=http://$ip:3000
  : > /tmp/reg; : > /tmp/login; : > /tmp/ready; bad=0
  for i in $(seq $n); do
    out=$(curl -s -m 30 -o /dev/null -w "%{http_code} %{time_total}" -d email=$label-$name-$i@example.com -d password=Correct-horse-battery-9 $B/auth/register)
    [ "${out%% *}" = 303 ] && echo "${out##* }" >> /tmp/reg || bad=$((bad+1))
    out=$(curl -s -m 30 -o /dev/null -w "%{http_code} %{time_total}" -d email=$label-$name-$i@example.com -d password=wrong-password-000 $B/auth/email/login)
    echo "${out##* }" >> /tmp/login
    curl -s -m 30 -o /dev/null -w "%{time_total}\n" $B/ready >> /tmp/ready
  done
  echo "$label $region $name register: $(stats < /tmp/reg), refused or failed $bad"
  echo "$label $region $name sign-in, wrong password: $(stats < /tmp/login)"
  echo "$label $region $name /ready: $(stats < /tmp/ready)"
done < /s/pods.txt
