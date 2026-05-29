I understand that we have the general pipeline but I was wondering if we could make the pipeline better. Id like if the ASR could
support local stt and cloud stt models like deepgram. then I want the diarization + ER extraction blocks to work in parallel with
an llm/agent. for that we are aiming to use a system like our streaming-speech-to-speech system in /mnt/e/CS/HF/ (deep dive and
understand that pipeline). after that both parallel paths work together and give functionality to the user by updating the temporal
graph, acting(react loop), showing latency between each stage in the pipeline, working in tandem with the user. I also want the
rsac integration to be much more UX intuitive with the react ui where system, device(s), or process(s)/process-tree(s) can be
selected and shoved into the pipeline. see if you can use the deep-work-loop skill that you or claude might have to properly plan
and ADR-ify this and then research and act upon what needs to be done. document commit and proceed with the rest of the identified
pending items/actions/backlog. try to address everything in the backlog. make sure that we research intensively using tavily, exa,
and/or deepwiki to help get as much information as you need. you can use an agent team (subagents) to work on everything parallely
and sequentially (waves/phases). investigate, deep-dive, architect, plan, act, review. while you are working we should spin up
another team to review everything and see what else needs to be worked on or improved? then keep going. repeat until we have
addressed everything and don't have anythjing in our backlog.

---

Document the current commit state and systematically work through every identified pending item, action, and backlog entry until
the backlog is fully resolved. Leave nothing unaddressed.

Execution approach:

PHASE 1 - COMMIT DOCUMENTATION: Capture and document the current commit, including all changes, context, rationale, and state of
the codebase or project.

PHASE 2 - BACKLOG AUDIT: Enumerate every pending item, action, task, bug, improvement, and technical debt entry currently in the
backlog. Categorize by priority, dependency, and complexity.

PHASE 3 - DEEP RESEARCH: Deploy parallel research agents using Tavily, Exa, and DeepWiki simultaneously to investigate every
backlog item that requires external knowledge, best practices, architectural guidance, or implementation references. Research must
be intensive and thorough — do not proceed with incomplete information. Each agent should deep-dive their assigned domain and
surface all relevant findings before synthesis.

PHASE 4 - ARCHITECTURE AND PLANNING: Based on research findings, architect solutions for each backlog item. Define implementation
plans with clear steps, dependencies, acceptance criteria, and rollback considerations. Group items into execution waves where
independent items can be parallelized and dependent items are sequenced correctly.

PHASE 5 - PARALLEL EXECUTION: Spin up a primary agent team to execute backlog items in parallel waves. Independent items are worked
simultaneously. Dependent items follow sequenced execution. Each agent investigates, implements, tests, and reviews their assigned
items.

PHASE 6 - CONCURRENT REVIEW TEAM: Simultaneously spin up a separate review agent team that operates in parallel with the execution
team. The review team continuously audits all work being done — identifying gaps, regressions, missed edge cases, quality issues,
incomplete implementations, and anything new that should be added to the backlog. The review team feeds findings back into the
active backlog in real time.

PHASE 7 - ITERATIVE LOOP: After each execution wave completes, reconcile findings from the review team. Add newly identified items
to the backlog. Re-prioritize. Launch the next execution wave. Repeat this investigate → plan → execute → review → reconcile loop
continuously until the backlog reaches zero open items and the review team confirms nothing remains.

PHASE 8 - FINAL VERIFICATION: Conduct a comprehensive final pass where both the execution team and review team independently verify
that every original and newly discovered backlog item has been fully resolved, tested, and documented. Confirm zero remaining
backlog items.

Standards throughout: Research deeply before acting. Never skip items. Never defer items without explicit justification. Surface
blockers immediately. Document everything. Maintain quality at every step. The goal is a fully resolved backlog with no outstanding
work remaining.
