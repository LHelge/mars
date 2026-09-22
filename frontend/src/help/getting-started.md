Mars runs coding agents for a repository, each session in a container of its own. You follow sessions in the browser, and the agents and the people on the project share one task board. Three steps take you from nothing to a working agent.

### 1. Create a project

On [Projects](/projects), choose **New project** and give it:

- a **name**;
- the repository's **remote URL**, over HTTPS and with no token in it;
- a **default branch**, or leave it empty and Mars finds it on the remote;
- a **credential** if the repository is private or you want Mars to push. This is a personal access token. [Git credential](help:git-credential) says which permissions it needs.

The project starts as `cloning` and turns `ready` once Mars has a copy of the repository. If the clone fails, the project shows the error and a **Retry clone** button.

### 2. Add an agent credential

Agents need something to authenticate with: a Claude subscription token or an Anthropic API key. Add one on the [Secrets](/secrets) page under **Add agent credential**. Choose the kind, paste the value and say who it applies to: you, one project or everyone. You never edit a profile for this. [Agent credentials](help:agent-credentials) explains which credential a session picks.

### 3. Launch a session

On the project's **Sessions** tab, pick a profile and choose **Launch session**. The project's default profile is already selected there. You can also give the session:

- a **first message**;
- a **task**, which the session then holds from the start;
- a **base ref**, if it should start from something other than the default branch.

The profile's system prompt tells the agent what its job is, and your message tells it what to do now. [How a session is instructed](help:instructions) explains how the two combine with a task.

The launch form shows which agent credential the session will use. If there is none, it offers **Add credential** instead.

A conversational session keeps running when nobody is watching. When it goes quiet it parks, and your next message picks it up where it stopped. The [Dashboard](/) lists every running and parked session, and every task that is waiting for a person.

### The starter profiles

A new project comes with a starter profile for each of its roles, and the project's **Profiles** tab lists them:

- **planner** works on the `backlog`: it turns a request into tasks an implementer can pick up, and never writes code.
- **implementer** works on `ready`: it claims a task, does the work and hands a commit over for review. This is the default profile.
- **reviewer** works on `review`: it checks the handed-over commit, then approves it or sends it back with concrete comments.

Each one is an ordinary profile with a system prompt written for its role. That copy belongs to your project, so you can change it freely. [Agent profiles](help:profiles) covers what a profile decides, and [Task flow](help:task-flow) shows how a task moves between these roles.

### Working with others

An administrator invites people from the **Admin** page. Each invite is an email link that is valid for seven days, and nobody can sign up without one. Everyone sees the same projects, boards and sessions. **Copy link** on a task or a session gives you a direct link to share, and whoever opens it has to sign in.
