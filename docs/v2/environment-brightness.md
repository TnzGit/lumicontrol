# Weather And Sun Brightness Model

LumiControl's **Weather & sun** source recommends monitor brightness without a
LumiControl sensor or relay. It is a deterministic, explainable local model,
not a cloud AI service. The only network input is current weather for the
configured coordinates.

## Inputs

- solar elevation, sunrise, sunset, and daylight duration
- day of year, latitude, and hemisphere for seasonal context
- weather kind, cloud cover, precipitation probability, and visibility
- the configured day peak and night target
- a personal offset from -50 to +50 percentage points

Solar values are calculated locally from the configured coordinates, time zone,
and current time. Current weather comes from
[Open-Meteo](https://open-meteo.com/en/docs) when available.

## Calculation

The model first maps solar elevation from civil twilight (-6 degrees) to a high
daytime sun (45 degrees) with a smootherstep curve. This gives continuous first
and second derivatives at both ends, so dawn and dusk do not create abrupt
target changes.

The daylight component is adjusted modestly for seasonal daylight duration.
Cloud, precipitation, and low visibility then reduce only that daylight
component. Night brightness therefore remains stable instead of reacting to a
weather label that has little meaning after dark.

Conceptually:

```text
sunlight = smootherstep(solar elevation)
daylight = sunlight * seasonal factor * weather factor
base = night target + (day peak - night target) * daylight
recommended = clamp(base + personal offset, 0, 100)
```

The Agent applies its existing deadband, target stabilization, and smooth DDC/CI
transition after calculating the recommendation. A large target change can be
completed in one retargetable transition; it does not wait through multiple
control intervals.

## Offline Behavior

If live weather cannot be fetched, LumiControl keeps running from the local
solar and seasonal model. The status page marks weather as unavailable rather
than freezing the previous display brightness. Weather is refreshed separately
from the 30-second solar recomputation, so the API is not polled on every control
tick.

## Privacy And Attribution

Weather requests include the configured latitude and longitude. They do not
include a LumiControl account, monitor identifier, USB identifier, or calibration
data. Open-Meteo data is licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); see
[`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md).
