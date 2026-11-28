mute() {
    local sink=victory_mute
    if ! pactl list short sinks | cut -f2 | grep -Fx "$sink" >/dev/null; then
        pactl load-module module-null-sink "sink_name=$sink" 'sink_properties=device.description=Victory-Mute' >/dev/null || return
    fi
    pactl set-sink-mute "$sink" 1 || return
    export PULSE_SINK="$sink"
    export WINEDLLOVERRIDES="winealsa.drv,wineoss.drv,winejack.drv=d${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
}
