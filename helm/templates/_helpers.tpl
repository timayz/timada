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
What a pod alone knows of itself: every pod reads the same configuration
file, so it comes from its environment. Its name is its region, unless
`config.server.region` gives every node the same one: the background work on
what a node created is then done by that node, and not once by each of them.
Among several nodes its ordinal is its place.
*/}}
{{- define "timada.nodeEnv" -}}
{{- $ownRegion := not (dig "server" "region" "" .Values.config) -}}
{{- $several := gt (include "timada.nodes" . | int) 1 -}}
{{- if or $ownRegion $several -}}
env:
  {{- if $ownRegion }}
  - name: TIMADA__SERVER__REGION
    valueFrom:
      fieldRef:
        fieldPath: metadata.name
  {{- end }}
  {{- if $several }}
  - name: TIMADA__CLUSTER__NODE_ID
    valueFrom:
      fieldRef:
        fieldPath: metadata.labels['apps.kubernetes.io/pod-index']
  {{- end }}
{{- end -}}
{{- end -}}
