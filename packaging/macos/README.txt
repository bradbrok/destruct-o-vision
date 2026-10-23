Destruct-O-Vision @VERSION@
A modular video synthesizer that emulates analog video.

INSTALL
Drag Destruct-O-Vision into Applications.

FIRST LAUNCH
This build isn't signed by Apple yet, so macOS blocks it the first time.
Open it once, then go to System Settings > Privacy & Security, scroll down,
and click "Open Anyway". On macOS 15 and later, right-click > Open no longer
gets around this.

GETTING STARTED
File > Demo patch, Analog demo, Delay & key demo, 3D & text demo and Audio
demo are good starting points. Right-click empty rack space to add modules,
and drag from jack to jack to patch. Your patches, snapshots and modules live
in Documents/Destruct-O-Vision.

SOUND AND MIDI
Audio In asks for microphone access the first time; click its top row to
pick any input and channel. MIDI Notes and MIDI CC listen to every MIDI
device, including ones plugged in later.

MOVIES
Drop movie files onto the rack to play them, or click a Movie module's top
row to choose a file. Click its picture to set In and Out to the frame and
to cut the movie into splices; Organize and Next pick the splice that plays.
Patches remember where the file is, so keep movies where they were when you
saved.

TEMPO
The tempo bar at the top right sets the tempo (drag, type or Tap) and shows
the beat. Choose Ableton Link to share tempo and beat with Live (turn Link on
in Live's settings), or MIDI clock to follow any device's clock (in Live,
turn Sync on for a MIDI output, such as the IAC Driver). The first time you
choose Link, macOS asks to let Destruct-O-Vision use the local network.

RECORDING
Output > Record movie (Cmd+R) records the output window, as it shows, to a
movie in Documents/Destruct-O-Vision/captures while you play. Cmd+R again
stops it. H.264 makes a small MP4; ProRes keeps every scan line for editing.

PICTURE TUBE
Output shows the picture clean. Put a CRT in its place to see it on a picture
tube: scan lines, phosphor mask, glow and curved glass. With Scan on Beam, a
display at 120 Hz (100 Hz for PAL) shows each field drawn top to bottom. Try
the Magnet, then Degauss.

EXAMPLE MODULES
Copy the folders in "Example Modules" into Documents/Destruct-O-Vision/modules
(Modules > Open modules folder). They load right away:
  example.starter   three effects (Line Tear, Bleed, Ghosts), with panel art
                    for Line Tear
  example.skins     panel art that restyles the built-in Folder and Mixer

WRITE YOUR OWN
Modules > New module... writes a starter module into the modules folder. Open
it in any text editor: it reloads every time you save, and the Modules menu
shows any errors. Panel art is an SVG named for the module, such as
panels/dov.vco.svg.

Free software under the GNU General Public License v3 or later (LICENSE.txt).
Source, issues and new releases: https://github.com/bradbrok/destruct-o-vision
