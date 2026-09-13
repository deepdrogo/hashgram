# Spaces

A Space is a shared environment — a family, a company, a project — with
roles. Under the hood it is one encrypted group plus a signed, hash-chained
log of events that every member replays to the same state. Nothing about
a Space is on chain and no node can read it.

## Roles

| Role | May |
| --- | --- |
| Guest | read |
| Member | post, comment, share files to the Space drive |
| Admin | everything above, plus edit info, announce, invite up to Admin (Admins only by the Owner), remove lower roles |
| Owner | everything, plus transfer ownership |

The rules are checked by **every** member's app, so an unauthorised event
is rejected by all of them. The app greys out what your role does not
allow; if the state raced, the action fails with the rule named.

## Overview, Posts, Drive, Members, Mail

**Overview** shows announcements (Admin+). **Posts** is the Space's
timeline. **Drive** lists files members shared into the Space, arranged
by the path they chose; *Live* shares follow the owner's edits.
**Members** manages roles. **Mail** sends one message to every member.

## Ownership

The Owner cannot leave or be removed. Transfer ownership from Members;
the previous Owner becomes an Admin and can then leave.
