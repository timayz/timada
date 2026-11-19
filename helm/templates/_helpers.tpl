{{/*
A pod's place among the nodes is its ordinal: every pod reads the same
configuration file, so the id comes from its environment.
*/}}
{{- define "timada.nodeEnv" -}}
{{- if gt (int .Values.replicas) 1 -}}
env:
  - name: TIMADA__CLUSTER__NODE_ID
    valueFrom:
      fieldRef:
        fieldPath: metadata.labels['apps.kubernetes.io/pod-index']
{{- end -}}
{{- end -}}
