program credits.aleo;

record credits:
    owner as address.private;
    gates as u64.private;

// validator_{n} is the tendermint public key (32 bytes)
// split into 4 u64s for compatibility with VM backends
// 0 is the bigger section, and 3 the lowest
record staked_credits:
    owner as address.private;
    gates as u64.private;
    validator_0 as u64.public; 
    validator_1 as u64.public; 
    validator_2 as u64.public; 
    validator_3 as u64.public; 

function transfer:
    input r0 as credits.record;
    input r1 as address.private;
    input r2 as u64.private;
    sub r0.gates r2 into r3;
    cast r1 r2 into r4 as credits.record;
    cast r0.owner r3 into r5 as credits.record;
    output r4 as credits.record;
    output r5 as credits.record;

function combine:
    input r0 as credits.record;
    input r1 as credits.record;
    add r0.gates r1.gates into r2;
    cast r0.owner r2 into r3 as credits.record;
    output r3 as credits.record;

function split:
    input r0 as credits.record;
    input r1 as u64.private;
    sub r0.gates r1 into r2;
    cast r0.owner r1 into r3 as credits.record;
    cast r0.owner r2 into r4 as credits.record;
    output r3 as credits.record;
    output r4 as credits.record;

function fee:
    input r0 as credits.record;
    input r1 as u64.private;
    sub r0.gates r1 into r2;
    cast r0.owner r2 into r3 as credits.record;
    output r3 as credits.record;

function stake:
    input r0 as credits.record;
    input r1 as u64.private;
    input r2 as u64.public;
    input r3 as u64.public;
    input r4 as u64.public;
    input r5 as u64.public;
    sub r0.gates r1 into r6;
    cast r0.owner r6 into r7 as credits.record;
    cast r0.owner r1 r2 r3 r4 r5 into r8 as staked_credits.record;
    add 0u64 r1 into r9;
    output r7 as credits.record;
    output r8 as staked_credits.record;
    output r9 as u64.public;
    output r7.owner as address.public;
    output r8.validator_0 as u64.public;
    output r8.validator_1 as u64.public;
    output r8.validator_2 as u64.public;
    output r8.validator_3 as u64.public;

function unstake:
    input r0 as staked_credits.record;
    input r1 as u64.private;
    sub r0.gates r1 into r2;
    cast r0.owner r2 r0.validator_0 r0.validator_1 r0.validator_2 r0.validator_3 into r3 as staked_credits.record;
    cast r0.owner r1 into r4 as credits.record;
    add 0u64 r1 into r5;
    output r4 as credits.record;
    output r3 as staked_credits.record;
    output r5 as u64.public;
    output r3.owner as address.public;
    output r3.validator_0 as u64.public;
    output r3.validator_1 as u64.public;
    output r3.validator_2 as u64.public;
    output r3.validator_3 as u64.public;