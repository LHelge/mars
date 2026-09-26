An agent starts every session with two kinds of instruction. **Standing instructions** come from the profile and the repository and are the same on every launch. The **first message** is what this one run is asked to do, and it depends on how the session was launched.

### Standing instructions

These are loaded every time the agent CLI starts, in a new session and whenever a parked session resumes:

1. **The CLI's own system prompt**, which Mars doesn't change.
2. **A fixed note from Mars on the session's git setup**, appended to it. It tells the agent that its working tree is a clone on its own `session/<id>` branch, that committing there is all it has to do because Mars fetches the branch back by itself, that there is no push from the container, and that getting work onto `main` or upstream is Mars's job, done through the [task flow](help:task-flow), a git tool or you. It is the same for every profile and can't be edited.
3. **The profile's system prompt**, after that note. It is read again on every start, so editing a profile also changes its parked sessions the next time they wake. A session that is running keeps the prompt it started with.
4. **The repository's own instructions**, from the session's checkout: its `CLAUDE.md`, its `.mcp.json` and its [skills](help:skills). Each session sees them as they are on the branch it started from. What earlier sessions of the project saved to the CLI's memory is loaded too.
5. **Mars's tool instructions**. They tell the agent that Mars is the task tracker for this session. If the repository's instructions name another tracker, the agent follows that workflow with Mars's tools instead and never writes task files into the repository.

### The first message

| Launched by | What the agent is told first |
| --- | --- |
| You, with no task | Your message. A conversational session without one starts and waits for you to write. An ephemeral session needs one. |
| You, for a task | A message Mars writes about the task, then your message if you gave one. |
| The dispatcher | The message Mars writes about the task it claimed, and nothing else. |
| A schedule | The profile's **schedule prompt**. There is no task, and the session starts from the default branch. |

The message Mars writes names the task and tells the agent to read it:

```
You hold task #12: Add retry to the webhook client. Call get_task to read it before starting.
```

If the task has a hand-off, a second paragraph names it: its id, the session and branch it came from, the commit, its review status and the hand-off comment. A session launched for such a task starts from that commit. If you chose a different **base ref**, a third paragraph says the checkout starts there and tells the agent where to fetch the hand-off commit from. [Task flow](help:task-flow) covers hand-offs, and [Automation](help:automation) the dispatcher and schedules.

### How the first message arrives

An **ephemeral** session gets one prompt and nothing after it. When there are two parts, they are joined with a blank line: the task message first, then yours.

A **conversational** session gets them as separate messages, in that order. The task message appears in the transcript like a message you sent, but nobody typed it. After that, the conversation is yours.

### What to write where

- **The system prompt** says who the agent is: its role, which tools it uses and how it hands work on. It stays the same for every session of the profile.
- **The message**, or a profile's **schedule prompt**, says what this run does.
- **The task** holds the details of the work. The agent reads it with `get_task`, including its description and comments, so don't paste the task into the message. Use the message for what the task doesn't say, such as "only the backend half today".
- **The repository** holds how work is done there: conventions, test commands and style rules belong in its `CLAUDE.md`, where every profile picks them up.
