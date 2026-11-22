{{/*
How many nodes run: one a zone where `zones` names some, `replicas` otherwise.
*/}}
{{- define "timada.nodes" -}}
{{- if .Values.zones -}}
{{- len .Values.zones -}}
{{- else -}}
{{- int .Values.replicas -}}
{{- end -}}
{{- end -}}

{{/*
Whether the nodes here are part of a cluster: there are several of them, or
`peers` names the others, which run elsewhere. Empty when not.
*/}}
{{- define "timada.clustered" -}}
{{- if or (gt (include "timada.nodes" . | int) 1) .Values.peers -}}
true
{{- end -}}
{{- end -}}

{{/*
The name the other nodes dial a pod of this release at, which its
certificate holds, as cert-manager's CSI driver takes it (`${POD_NAME}`
stands for the pod's name): `timada-<i>.timada-headless` inside one release,
and with `peers` the release's entries, which must all be
`timada-<i>.<one rest>`.
*/}}
{{- define "timada.dialledAs" -}}
{{- if .Values.peers -}}
{{- $first := int (.Values.firstNodeId | default 0) -}}
{{- $prefix := printf "timada-%d." $first -}}
{{- $host := regexReplaceAll ":[0-9]+$" (index .Values.peers $first) "" -}}
{{- if not (hasPrefix $prefix $host) -}}
{{- fail (printf "peers[%d] is %s: this release's pods are dialled as timada-<number>.<a name the same for each>, which their certificates hold" $first $host) -}}
{{- end -}}
{{- $rest := trimPrefix $prefix $host -}}
{{- range $i := until (include "timada.nodes" $ | int) -}}
{{- $id := add $first $i -}}
{{- $host := regexReplaceAll ":[0-9]+$" (index $.Values.peers $id) "" -}}
{{- if ne $host (printf "timada-%d.%s" $id $rest) -}}
{{- fail (printf "peers[%d] is %s, where this release's pod %d is timada-%d.%s, as the others of the release: a pod's certificate holds its own name and that rest" $id $host $id $id $rest) -}}
{{- end -}}
{{- end -}}
${POD_NAME}.{{ $rest }}
{{- else -}}
${POD_NAME}.timada-headless
{{- end -}}
{{- end -}}

{{/*
The configuration file a pod reads: the one of its own name where
`workGroups` gives each pod a work group (`$(POD_NAME)` is the pod's name,
from its environment), the same for all otherwise.
*/}}
{{- define "timada.configFile" -}}
{{- if .Values.workGroups -}}
/etc/timada/$(POD_NAME).toml
{{- else -}}
/etc/timada/config.toml
{{- end -}}
{{- end -}}

{{/*
What a pod alone knows of itself, which comes from its environment. Its name
is its work group, unless `config.server.work_group` gives every node the
same one or `workGroups` names each pod's (its name then picks its
configuration file): the background work on what a node created is then done
by that node, and not once by each of them. In a cluster its ordinal is its
place, which starts at `firstNodeId`.
*/}}
{{- define "timada.nodeEnv" -}}
{{- $named := .Values.workGroups -}}
{{- $ownGroup := and (not $named) (not (dig "server" "work_group" "" .Values.config)) -}}
{{- $clustered := include "timada.clustered" . -}}
{{- if or $ownGroup $clustered $named -}}
env:
  {{- if $named }}
  - name: POD_NAME
    valueFrom:
      fieldRef:
        fieldPath: metadata.name
  {{- end }}
  {{- if $ownGroup }}
  - name: TIMADA__SERVER__WORK_GROUP
    valueFrom:
      fieldRef:
        fieldPath: metadata.name
  {{- end }}
  {{- if $clustered }}
  - name: TIMADA__CLUSTER__NODE_ID
    valueFrom:
      fieldRef:
        fieldPath: metadata.labels['apps.kubernetes.io/pod-index']
  {{- end }}
{{- end -}}
{{- end -}}
