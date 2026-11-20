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
