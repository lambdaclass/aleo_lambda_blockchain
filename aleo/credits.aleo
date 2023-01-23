program credits.aleo;

record credits:
    owner as address.private;
    gates as u64.private;

record staked_credits:
    owner as address.private;
    gates as u64.private;
    validator_higher as u128.public; 
    validator_lower as u128.public; 

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
    // validator address higher part
    input r2 as u128.public;
    // validator address lower part
    input r3 as u128.public;
    sub r0.gates r1 into r4;
    cast r0.owner r4 into r5 as credits.record;
    cast r0.owner r1 r2 r3 into r6 as staked_credits.record;
    add 0u64 r1 into r7;
    output r5 as credits.record;
    output r6 as staked_credits.record;
    output r7 as u64.public;
    output r5.owner as address.public;
    output r6.validator_higher as u128.public;
    output r6.validator_lower as u128.public;

function unstake:
    input r0 as staked_credits.record;
    input r1 as u64.private;
    sub r0.gates r1 into r2;
    cast r0.owner r2 r0.validator_higher r0.validator_lower into r3 as staked_credits.record;
    cast r0.owner r1 into r4 as credits.record;
    add 0u64 r1 into r5;
    output r4 as credits.record;
    output r3 as staked_credits.record;
    output r5 as u64.public;
    output r3.owner as address.public;
    output r3.validator_higher as u128.public;
    output r3.validator_lower as u128.public;
