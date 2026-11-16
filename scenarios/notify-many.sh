#!/bin/sh
# More notifications than the list shows at once, from several applications: some stack, one has a long body and
# one an action.
send() {
    notify-send --app-name="$1" "$2" "$3"
}
send Files "Download finished" "Holiday photos.zip is in Downloads."
send Calendar "Stand-up" "In 5 minutes, in the small meeting room."
send Calendar "Design review" "In 30 minutes."
send Mail "Release notes draft" "Here is the draft of the release notes for the next version. The first section covers the notification list, which now scrolls with the wheel, the touchpad and the keyboard. Please read it to the end and send me your comments by tomorrow afternoon."
send Weather "Rain from 16:00" "Light rain until the evening, 14 degrees."
send Backup "Backup finished" "42 files copied in 3 minutes."
send Music "Now playing" "Morning Walk, by the Quiet Hours."
send Notes "Shared with you" "Packing list for the trip."
send Photos "Import finished" "128 pictures from the camera."
send Updates "3 updates are ready" "They install the next time you restart."
send Printer "Document printed" "Boarding pass.pdf, 2 pages."
send Chat "Ana" "Are you free for lunch at noon?"
send Chat "Ana" "The usual place?"
exec notify-send --app-name=Reminders --action=done=Done "Water the plants" "Every Monday."
