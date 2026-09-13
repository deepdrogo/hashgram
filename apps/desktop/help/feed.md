# Feed and Circles

## Feed

Public posts are signed events fetched from nodes and verified against
the author's device keys on chain. There is no ranking algorithm:
**Friends** and **Following** are chronological. **Explore** needs a
public indexer URL (Settings → Network); without one the tab explains
why it is off.

Posts take text, up to 20 media files and hashtags; a *sensitive* flag
hides media until clicked. You can edit and delete your own posts;
deletion is a tombstone event that every reader honours.

## Circles

A Circle is a private group: every post travels encrypted to the members'
devices and nowhere else. Adding a member starts a new encryption epoch,
so they do not see earlier posts. Removing a member cuts them off from
the next post on.

Circle posts can carry a **poll**: 2–12 options, single or multiple
choice, with an optional closing time. Votes are counted on each member's
device from the events they received.

Circles have no roles: any member may add members. Use a **Space** when
you need administrators and read-only guests.
