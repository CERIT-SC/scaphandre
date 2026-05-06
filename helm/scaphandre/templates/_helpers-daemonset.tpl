{{- define "scaphandre.daemonset.spec" -}}
{{- $ctx := .context -}}
{{- $gpuPresent := .gpuPresent -}}
spec:
  updateStrategy:
    type: RollingUpdate
  selector:
    matchLabels:
      app.kubernetes.io/name: {{ include "scaphandre.name" $ctx }}
  template:
    metadata:
      name: {{ include "scaphandre.name" $ctx }}
      labels:
        {{- include "labels.common" $ctx | nindent 8 }}
      {{- if .Values.podAnnotations }}
      annotations:
        {{- toYaml .Values.podAnnotations | nindent 8 }}
      {{- end }}
    spec:
      hostPID: {{ .Values.hostPID | default true }}
      {{- if $gpuPresent }}
      runtimeClassName: nvidia
      {{- end }}
      {{- if .Values.gpuSupport.enabled }}
      affinity:
        nodeAffinity:
          requiredDuringSchedulingIgnoredDuringExecution:
            nodeSelectorTerms:
            - matchExpressions:
              - key: nvidia.com/gpu.present
                operator: {{ if $gpuPresent }}In{{ else }}NotIn{{ end }}
                values:
                - "true"
      {{- end }}
      containers:
      - name: {{ include "scaphandre.name" $ctx }}
        image: "{{ .Values.image.name }}:{{ .Values.image.tag }}"
        imagePullPolicy: "{{ .Values.image.pullPolicy }}"
        args:
        {{- range .Values.scaphandre.args }}
        - {{ . }}
        {{- end }}
        {{- if or .Values.scaphandre.rustBacktrace $gpuPresent }}
        env:
        {{- if .Values.scaphandre.rustBacktrace }}
        - name: RUST_BACKTRACE
          value: '{{ .Values.scaphandre.rustBacktrace }}'
        {{- end }}
        {{- if $gpuPresent }}
        - name: NVIDIA_VISIBLE_DEVICES
          value: all
        {{- end }}
        {{- end }}
        ports:
        - name: metrics
          containerPort: {{ .Values.port }}
        resources:
{{ toYaml .Values.resources | indent 10 }}
        volumeMounts:
        - mountPath: /proc
          name: proc
          readOnly: true
        - mountPath: /sys/class/powercap
          name: powercap
          readOnly: true
        {{- if $gpuPresent }}
        securityContext:
          privileged: true
        {{- end }}
      securityContext:
        runAsUser: {{ .Values.userID }}
        runAsGroup: {{ .Values.userGroup }}
      {{- if .Values.scaphandre.containers }}
      serviceAccountName: {{ include "scaphandre.name" $ctx }}
      {{- end }}
      tolerations:
      # Tolerate all taints for observability
      - operator: "Exists"
      volumes:
      - hostPath:
          path: /proc
          type: "Directory"
        name: proc
      - hostPath:
          path: /sys/class/powercap
          type: "Directory"
        name: powercap
{{- end -}}
