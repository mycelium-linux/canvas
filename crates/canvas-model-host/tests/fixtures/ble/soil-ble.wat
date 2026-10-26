;; Re-export the soil:ble/ble import surface under root names so tests can call
;; every verb through a real component instance. The Soil linker registers the
;; import; this fixture adds no logic and no ambient authority of its own.
;; Regenerate the .wasm with:
;;   cargo run -p canvas-model-host --example build-wasm-fixtures --features soil-ble
(component
  (type $ble (instance
    (type $device (record
      (field "handle" u32)
      (field "name" (option string))
      (field "rssi" (option s16))
      (field "ring-id" (option string))))
    (type $characteristic (record
      (field "uuid" string)
      (field "readable" bool)
      (field "writable" bool)
      (field "notifiable" bool)))
    (type $service (record
      (field "uuid" string)
      (field "characteristics" (list $characteristic))))
    (type $notification-payload (record
      (field "char-uuid" string)
      (field "data" (list u8))))
    (type $disconnect-reason (record (field "reason" string)))
    (type $event (variant
      (case "notification" $notification-payload)
      (case "disconnected" $disconnect-reason)))
    (type $error (enum
      "denied" "not-found" "not-connected" "leased" "timeout"
      "over-budget" "not-ready" "invalid" "io"))
    (export "device" (type (eq $device)))
    (export "characteristic" (type (eq $characteristic)))
    (export "service" (type (eq $service)))
    (export "notification-payload" (type (eq $notification-payload)))
    (export "disconnect-reason" (type (eq $disconnect-reason)))
    (export "event" (type (eq $event)))
    (export "error" (type (eq $error)))
    (export "scan" (func (param "timeout-ms" u32)
      (result (result (list $device) (error $error)))))
    (export "connect" (func (param "device" u32)
      (result (result (error $error)))))
    (export "disconnect" (func (result (result (error $error)))))
    (export "services" (func (result (result (list $service) (error $error)))))
    (export "read" (func (param "char-uuid" string) (param "max-bytes" u32)
      (result (result (list u8) (error $error)))))
    (export "write" (func (param "char-uuid" string) (param "data" (list u8))
      (result (result (error $error)))))
    (export "subscribe" (func (param "char-uuid" string)
      (result (result (error $error)))))
    (export "next-event" (func (param "timeout-ms" u32)
      (result (result (option $event) (error $error)))))))
  (import "soil:ble/ble@1.0.0" (instance $ble-import (type $ble)))
  (alias export $ble-import "scan" (func $scan))
  (alias export $ble-import "connect" (func $connect))
  (alias export $ble-import "disconnect" (func $disconnect))
  (alias export $ble-import "services" (func $services))
  (alias export $ble-import "read" (func $read))
  (alias export $ble-import "write" (func $write))
  (alias export $ble-import "subscribe" (func $subscribe))
  (alias export $ble-import "next-event" (func $next-event))
  (export "scan" (func $scan))
  (export "connect" (func $connect))
  (export "disconnect" (func $disconnect))
  (export "services" (func $services))
  (export "read" (func $read))
  (export "write" (func $write))
  (export "subscribe" (func $subscribe))
  (export "next-event" (func $next-event))
)
