program records.aleo;

record token:
     owner as address.private;
     gates as u64.private;
     amount as u64.public;

function mint:
     input r0 as u64.public;
     input r1 as address.public;
     cast r1 0u64 r0 into r2 as token.record;      
     output r2 as token.record;

function consume:
     input r0 as token.record;
     add r0.amount 0u64 into r1;
     output r1 as u64.public;

function consume_b:
     input r0 as token.record;
     add r0.amount 0u64 into r1;
     output r1 as u64.public;

function consume_two:
     input r0 as token.record;
     input r1 as token.record;
     add r0.amount 0u64 into r2;
     add r1.amount 0u64 into r3;
     output r2 as u64.public;

// this function is mainly used for testing that this is not possible
function mint_credits:
     input r0 as u64.public;
     input r1 as address.public;
     cast r1 r0 r0 into r2 as token.record;      
     output r2 as token.record;
