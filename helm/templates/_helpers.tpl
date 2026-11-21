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
What a pod alone knows of itself: every pod reads the same configuration
file, so it comes from its environment. Its name is its work group, unless
`config.server.work_group` gives every node the same one: the background work on
what a node created is then done by that node, and not once by each of them.
In a cluster its ordinal is its place, which starts at `firstNodeId`.
*/}}
{{- define "timada.nodeEnv" -}}
{{- $ownGroup := not (dig "server" "work_group" "" .Values.config) -}}
{{- $clustered := include "timada.clustered" . -}}
{{- if or $ownGroup $clustered -}}
env:
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
