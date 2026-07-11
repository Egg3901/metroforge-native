# MetroForge v0.5.0 Release Notes

## The city comes alive at night.

See your transit network glow. Street lamps bloom across the grid. Headlights trace vehicle paths. Buildings light their windows floor by floor as the sun sets behind the procedural facades. Water ripples under your transit lines, catching the light.

## What's New

**Night and Detail**

The core city now has visual depth. Every building gets procedural windows, rooftops, and an intricate LOD system that lets you zoom from a distant glowing silhouette to individual lit panes. Street lights and vehicle headlights cast bloom. The water shader respects the terrain and reflects the sky.

**Navigate with the Minimap**

Press N to toggle the minimap. Click any district to pan the camera there. Hover over routes to see them dimmed globally, making crowded networks readable without clicking individual lines. The minimap caches its layers for smooth panning even in large cities.

**Learn the Game**

New players now get a guided tutorial on first launch. Objectives appear in-game as goals, progressing tier by tier. Tutorial steps cover the basics: building your first station, serving demand, watching the economy respond.

**Safer Saves**

Autosave now runs in a versioned ring of 3 save slots, giving you a 5-10 minute buffer to recover from accidental deletes. Save slots show metadata on the load screen. Schema versioning means future updates won't break your existing games.

**See What the Sim is Thinking**

Building panels now show the economic layers the sidecar sees: demand flows, utilization, supply. The economy isn't magic anymore. Watch a station fulfill demand in real time or see why a route is failing.

**Performance**

Builds are faster. Rendering is cleaner on the potato tiers, with proper draw-distance fog masking pop-in. The Windows version now feels like a desktop citizen: remember your window size and position, pause rendering when alt-tabbed, minimize smoothly without hitches.

## Known Limitations

Streetless render persists from v0.4.4. Photo mode and graphics settings UI will ship in a follow-up.

## Download

[Linux, Windows, macOS builds](https://github.com/egg3901/metroforge-native/releases/tag/v0.5.0-alpha)
